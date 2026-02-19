use std::str::FromStr;

use bls::PublicKeyBytes;
use serde::Deserialize;
use types::{ChainSpec, DepositMessage, Domain, Hash256, SignedRoot};

use crate::SpecTest;

/// Maximum effective balance for a standard deposit (32 ETH in Gwei).
/// Matches Go's `types.MaxEffectiveBalanceInGwei`.
const MAX_EFFECTIVE_BALANCE_GWEI: u64 = 32_000_000_000;

/// Tests deposit data signing root computation.
///
/// Exercises Lighthouse's deposit primitives (`DepositMessage`, `ChainSpec::compute_domain`,
/// `SignedRoot::signing_root`) rather than Anchor-specific logic. Confirms cross-implementation
/// compatibility with the Go SSV spec's `GenerateETHDepositData()`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BeaconDepositDataSpecTest {
    #[expect(dead_code)]
    name: String,
    #[serde(rename = "ValidatorPK")]
    validator_pk: String,
    withdrawal_credentials: String,
    fork_version: String,
    expected_signing_root: String,
    #[expect(dead_code)]
    domain: String,
}

impl SpecTest for BeaconDepositDataSpecTest {
    fn run(&self) -> Result<(), String> {
        // Parse validator public key (fixture has no 0x prefix)
        let pubkey = PublicKeyBytes::from_str(&format!("0x{}", self.validator_pk))
            .map_err(|e| format!("Failed to parse ValidatorPK: {e}"))?;

        // Parse withdrawal credentials
        let wc_bytes = hex::decode(&self.withdrawal_credentials)
            .map_err(|e| format!("Failed to decode WithdrawalCredentials: {e}"))?;
        let withdrawal_credentials = Hash256::from_slice(&wc_bytes);

        // Parse fork version
        let fv_bytes = hex::decode(&self.fork_version)
            .map_err(|e| format!("Failed to decode ForkVersion: {e}"))?;
        let fork_version: [u8; 4] = fv_bytes
            .try_into()
            .map_err(|_| "ForkVersion must be 4 bytes".to_string())?;

        // Construct DepositMessage
        let deposit_msg = DepositMessage {
            pubkey,
            withdrawal_credentials,
            amount: MAX_EFFECTIVE_BALANCE_GWEI,
        };

        // Compute domain and signing root
        let spec = ChainSpec::mainnet();
        let domain = spec.compute_domain(Domain::Deposit, fork_version, Hash256::ZERO);
        let signing_root = deposit_msg.signing_root(domain);

        // Compare
        let expected_bytes = hex::decode(&self.expected_signing_root)
            .map_err(|e| format!("Failed to decode ExpectedSigningRoot: {e}"))?;
        let expected = Hash256::from_slice(&expected_bytes);

        if signing_root != expected {
            return Err(format!(
                "Signing root mismatch: got {}, expected {}",
                hex::encode(signing_root.as_slice()),
                self.expected_signing_root,
            ));
        }
        Ok(())
    }
}
