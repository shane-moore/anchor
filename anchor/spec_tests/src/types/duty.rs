use serde::Deserialize;
use ssv_types::consensus::{
    BEACON_ROLE_AGGREGATOR, BEACON_ROLE_ATTESTER, BEACON_ROLE_PROPOSER, BEACON_ROLE_SYNC_COMMITTEE,
    BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION, BEACON_ROLE_VALIDATOR_REGISTRATION,
    BEACON_ROLE_VOLUNTARY_EXIT, BeaconRole,
};

use crate::SpecTest;

/// Maps a beacon role to its expected duty role integer.
/// Mirrors Go's `MapDutyToRunnerRole()` from `types/beacon_types.go`.
/// Spec-test-only — Anchor dispatches duties through separate code paths
/// that already know their role, so this mapping isn't needed in production.
fn map_beacon_role_to_duty_role(beacon_role: BeaconRole) -> i32 {
    match beacon_role {
        BEACON_ROLE_ATTESTER | BEACON_ROLE_SYNC_COMMITTEE => 0,
        BEACON_ROLE_PROPOSER => 2,
        BEACON_ROLE_AGGREGATOR | BEACON_ROLE_SYNC_COMMITTEE_CONTRIBUTION => 6,
        BEACON_ROLE_VALIDATOR_REGISTRATION => 4,
        BEACON_ROLE_VOLUNTARY_EXIT => 5,
        _ => -1,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DutySpecTest {
    name: String,
    beacon_role: u64,
    #[serde(rename = "RunnerRole")]
    expected_duty_role: i32,
}

impl SpecTest for DutySpecTest {
    fn run(&self) -> Result<(), String> {
        let result = map_beacon_role_to_duty_role(BeaconRole::from(self.beacon_role));
        if result != self.expected_duty_role {
            return Err(format!(
                "BeaconRole({}) mapped to {result}, expected {}",
                self.beacon_role, self.expected_duty_role
            ));
        }
        Ok(())
    }
}
