// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mode_management::{Defect, EventHistory, IfaceFailure, PhyFailure};
use crate::telemetry;
use fuchsia_inspect::Node as InspectNode;
use fuchsia_inspect_contrib::inspect_insert;
use fuchsia_inspect_contrib::log::WriteInspect;
use futures::channel::mpsc;
use log::warn;
use std::borrow::Cow;

// As a general note, recovery is intended to be a method of last resort.  It should be used in
// circumstances where it is thought that WLAN firmware or the interface with the WLAN peripheral
// are not working properly.

// To ensure that devices are not constantly recovering, throttle recovery interventions by
// ensuring that PHY resets are only recommended every 24 hours and interface destructions are only
// recommended every 12 hours.
const HOURS_BETWEEN_PHY_RESETS: i64 = 24;
const HOURS_BETWEEN_IFACE_DESTRUCTIONS: i64 = 12;

// The following constants were empirically determined by looking over aggregate fleet metrics of
// device-day counts of events that represent unexpected device behavior.  Devices are allowed to
// encounter a number of events up to these thresholds before some recovery intervention may be
// recommended.
const SCAN_FAILURE_RECOVERY_THRESHOLD: usize = 5;
const EMPTY_SCAN_RECOVERY_THRESHOLD: usize = 10;
const CANCELED_SCAN_RECOVERY_THRESHOLD: usize = 9;
pub(crate) const CONNECT_FAILURE_RECOVERY_THRESHOLD: usize = 15;
pub(crate) const AP_START_FAILURE_RECOVERY_THRESHOLD: usize = 14;
const CREATE_IFACE_FAILURE_RECOVERY_THRESHOLD: usize = 1;
const DESTROY_IFACE_FAILURE_RECOVERY_THRESHOLD: usize = 1;

#[derive(Clone, Copy, Debug)]
pub enum PhyRecoveryOperation {
    DestroyIface { iface_id: u16 },
    ResetPhy { phy_id: u16 },
}

impl PartialEq for PhyRecoveryOperation {
    fn eq(&self, other: &Self) -> bool {
        match *self {
            PhyRecoveryOperation::DestroyIface { .. } => {
                matches!(*other, PhyRecoveryOperation::DestroyIface { .. })
            }
            PhyRecoveryOperation::ResetPhy { .. } => {
                matches!(*other, PhyRecoveryOperation::ResetPhy { .. })
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum IfaceRecoveryOperation {
    Disconnect { iface_id: u16 },
    StopAp { iface_id: u16 },
}

impl PartialEq for IfaceRecoveryOperation {
    fn eq(&self, other: &Self) -> bool {
        match *self {
            IfaceRecoveryOperation::Disconnect { .. } => {
                matches!(*other, IfaceRecoveryOperation::Disconnect { .. })
            }
            IfaceRecoveryOperation::StopAp { .. } => {
                matches!(*other, IfaceRecoveryOperation::StopAp { .. })
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RecoveryAction {
    PhyRecovery(PhyRecoveryOperation),
    IfaceRecovery(IfaceRecoveryOperation),
}

impl RecoveryAction {
    fn as_phy_recovery_mechanism(self) -> Option<telemetry::PhyRecoveryMechanism> {
        match self {
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { .. }) => {
                Some(telemetry::PhyRecoveryMechanism::PhyReset)
            }
            RecoveryAction::IfaceRecovery(..)
            | RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { .. }) => None,
        }
    }

    fn as_ap_recovery_mechanism(self) -> Option<telemetry::ApRecoveryMechanism> {
        match self {
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { .. }) => {
                Some(telemetry::ApRecoveryMechanism::DestroyIface)
            }
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { .. }) => {
                Some(telemetry::ApRecoveryMechanism::ResetPhy)
            }
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp { .. }) => {
                Some(telemetry::ApRecoveryMechanism::StopAp)
            }
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect { .. }) => None,
        }
    }

    fn as_client_recovery_mechanism(self) -> Option<telemetry::ClientRecoveryMechanism> {
        match self {
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { .. }) => {
                Some(telemetry::ClientRecoveryMechanism::DestroyIface)
            }
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { .. }) => {
                Some(telemetry::ClientRecoveryMechanism::PhyReset)
            }
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect { .. }) => {
                Some(telemetry::ClientRecoveryMechanism::Disconnect)
            }
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp { .. }) => None,
        }
    }

    fn as_timeout_recovery_mechanism(self) -> Option<telemetry::TimeoutRecoveryMechanism> {
        match self {
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { .. }) => {
                Some(telemetry::TimeoutRecoveryMechanism::PhyReset)
            }
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { .. }) => {
                Some(telemetry::TimeoutRecoveryMechanism::DestroyIface)
            }
            RecoveryAction::IfaceRecovery(..) => None,
        }
    }
}

impl WriteInspect for RecoveryAction {
    fn write_inspect<'a>(&self, writer: &InspectNode, key: impl Into<Cow<'a, str>>) {
        match self {
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id }) => {
                inspect_insert!(writer, var key: {DestroyIface: {iface_id: iface_id}})
            }
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id }) => {
                inspect_insert!(writer, var key: {ResetPhy: {phy_id: phy_id}})
            }
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect { iface_id }) => {
                inspect_insert!(writer, var key: {Disconnect: {iface_id: iface_id}})
            }
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp { iface_id }) => {
                inspect_insert!(writer, var key: {StopAp: {iface_id: iface_id}})
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecoverySummary {
    pub defect: Defect,
    pub action: RecoveryAction,
}

impl RecoverySummary {
    pub fn new(defect: Defect, action: RecoveryAction) -> Self {
        RecoverySummary { defect, action }
    }

    pub fn as_recovery_reason(self) -> Option<telemetry::RecoveryReason> {
        // Construct the associated metric and its dimension.
        match self.defect {
            Defect::Phy(PhyFailure::IfaceCreationFailure { .. }) => {
                Some(telemetry::RecoveryReason::CreateIfaceFailure(
                    self.action.as_phy_recovery_mechanism()?,
                ))
            }
            Defect::Phy(PhyFailure::IfaceDestructionFailure { .. }) => {
                Some(telemetry::RecoveryReason::DestroyIfaceFailure(
                    self.action.as_phy_recovery_mechanism()?,
                ))
            }
            Defect::Iface(IfaceFailure::Timeout { .. }) => Some(
                telemetry::RecoveryReason::Timeout(self.action.as_timeout_recovery_mechanism()?),
            ),
            Defect::Iface(IfaceFailure::ApStartFailure { .. }) => Some(
                telemetry::RecoveryReason::StartApFailure(self.action.as_ap_recovery_mechanism()?),
            ),
            Defect::Iface(IfaceFailure::CanceledScan { .. }) => {
                Some(telemetry::RecoveryReason::ScanCancellation(
                    self.action.as_client_recovery_mechanism()?,
                ))
            }
            Defect::Iface(IfaceFailure::FailedScan { .. }) => Some(
                telemetry::RecoveryReason::ScanFailure(self.action.as_client_recovery_mechanism()?),
            ),
            Defect::Iface(IfaceFailure::EmptyScanResults { .. }) => {
                Some(telemetry::RecoveryReason::ScanResultsEmpty(
                    self.action.as_client_recovery_mechanism()?,
                ))
            }
            Defect::Iface(IfaceFailure::ConnectionFailure { .. }) => {
                Some(telemetry::RecoveryReason::ConnectFailure(
                    self.action.as_client_recovery_mechanism()?,
                ))
            }
        }
    }
}

impl WriteInspect for RecoverySummary {
    fn write_inspect<'a>(&self, writer: &InspectNode, key: impl Into<Cow<'a, str>>) {
        inspect_insert!(writer, var key: {
            defect: self.defect,
            action: self.action,
        })
    }
}

pub const RECOVERY_SUMMARY_CHANNEL_CAPACITY: usize = 100;
pub(crate) type RecoveryActionSender = mpsc::Sender<RecoverySummary>;
pub(crate) type RecoveryActionReceiver = mpsc::Receiver<RecoverySummary>;

// The purpose of a RecoveryProfile function is to look at the most-recently observed defect in the
// context of past defects that have been encountered and past recovery actions that have been
// taken and suggest a possible recovery action to take to remedy the most recent defect.
pub type RecoveryProfile = fn(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    latest_defect: Defect,
) -> Option<RecoveryAction>;

// This is available so that new products' behaviors can be characterized before enforcing any
// recovery thresholds.  This will enable finding real bugs in a device's behavior.
fn recovery_disabled(
    _phy_id: u16,
    _defect_history: &mut EventHistory<Defect>,
    _recovery_history: &mut EventHistory<RecoveryAction>,
    _latest_defect: Defect,
) -> Option<RecoveryAction> {
    None
}

// This recovery profile takes into account the various defect thresholds and remediation limits to
// recommend a recovery solution when defects are encountered.
fn thresholded_recovery(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    latest_defect: Defect,
) -> Option<RecoveryAction> {
    match latest_defect {
        Defect::Phy(PhyFailure::IfaceCreationFailure { .. }) => {
            thresholded_create_iface_failure_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Phy(PhyFailure::IfaceDestructionFailure { .. }) => {
            thresholded_destroy_iface_failure_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Iface(IfaceFailure::CanceledScan { .. }) => {
            thresholded_canceled_scan_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Iface(IfaceFailure::FailedScan { .. }) => {
            thresholded_scan_failure_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Iface(IfaceFailure::EmptyScanResults { .. }) => {
            thresholded_empty_scan_results_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Iface(IfaceFailure::ApStartFailure { .. }) => {
            thresholded_ap_start_failure_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Iface(IfaceFailure::ConnectionFailure { .. }) => {
            thresholded_connect_failure_recovery_profile(
                phy_id,
                defect_history,
                recovery_history,
                latest_defect,
            )
        }
        Defect::Iface(IfaceFailure::Timeout { .. }) => {
            // TODO(b/42066276): Determine thresholds and recovery options.
            None
        }
    }
}

// Enable the lookup of recovery profiles by description.
pub fn lookup_recovery_profile(profile_name: &str) -> RecoveryProfile {
    match profile_name {
        "" => recovery_disabled,
        "thresholded_recovery" => thresholded_recovery,
        other => {
            warn!("Invalid recovery profile: {}.  Proceeding with default.", other);
            recovery_disabled
        }
    }
}

fn thresholded_iface_destruction_and_phy_reset(
    phy_id: u16,
    iface_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    most_recent_defect: Defect,
    defect_count_threshold: usize,
) -> Option<RecoveryAction> {
    let proposed_iface_destruction_action =
        RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id });

    if recovery_history.event_count(proposed_iface_destruction_action) > 0
        && let Some(phy_reset_recovery) = thresholded_phy_reset(
            phy_id,
            defect_history,
            recovery_history,
            most_recent_defect,
            defect_count_threshold,
        )
    {
        return Some(phy_reset_recovery);
    }

    thresholded_iface_destruction(
        iface_id,
        defect_history,
        recovery_history,
        most_recent_defect,
        defect_count_threshold,
    )
}

fn thresholded_phy_reset(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    most_recent_defect: Defect,
    defect_count_threshold: usize,
) -> Option<RecoveryAction> {
    let proposed_phy_reset_action =
        RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id });

    if defect_history.event_count(most_recent_defect) < defect_count_threshold {
        return None;
    }

    // If the threshold has been crossed and sufficient time has passed since the last PHY reset,
    // recommend that the PHY be reset.
    let recovery_allowed = match recovery_history.time_since_last_event(proposed_phy_reset_action) {
        None => true,
        Some(time) => time.into_hours() > HOURS_BETWEEN_PHY_RESETS,
    };

    if recovery_allowed {
        return Some(proposed_phy_reset_action);
    }

    None
}

fn thresholded_iface_destruction(
    iface_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    most_recent_defect: Defect,
    defect_count_threshold: usize,
) -> Option<RecoveryAction> {
    let proposed_iface_destruction_action =
        RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id });

    if defect_history.event_count(most_recent_defect) < defect_count_threshold {
        return None;
    }

    // If the threshold has been crossed and sufficient time has passed since the last iface
    // destruction, recommend that the iface be destroyed.
    let recovery_allowed =
        match recovery_history.time_since_last_event(proposed_iface_destruction_action) {
            None => true,
            Some(time) => time.into_hours() > HOURS_BETWEEN_IFACE_DESTRUCTIONS,
        };

    if recovery_allowed {
        return Some(proposed_iface_destruction_action);
    }

    None
}

fn thresholded_scan_failure_recovery_profile(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    scan_failure_defect: Defect,
) -> Option<RecoveryAction> {
    let iface_id = match scan_failure_defect {
        Defect::Iface(IfaceFailure::FailedScan { iface_id }) => iface_id,
        other => {
            warn!("Assessing invalid defect type for scan failure recovery: {:?}", other);
            return None;
        }
    };

    thresholded_iface_destruction_and_phy_reset(
        phy_id,
        iface_id,
        defect_history,
        recovery_history,
        scan_failure_defect,
        SCAN_FAILURE_RECOVERY_THRESHOLD,
    )
}

fn thresholded_empty_scan_results_recovery_profile(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    empty_scan_defect: Defect,
) -> Option<RecoveryAction> {
    let iface_id = match empty_scan_defect {
        Defect::Iface(IfaceFailure::EmptyScanResults { iface_id }) => iface_id,
        other => {
            warn!("Assessing invalid defect type for empty scan results recovery: {:?}", other);
            return None;
        }
    };

    thresholded_iface_destruction_and_phy_reset(
        phy_id,
        iface_id,
        defect_history,
        recovery_history,
        empty_scan_defect,
        EMPTY_SCAN_RECOVERY_THRESHOLD,
    )
}

fn thresholded_connect_failure_recovery_profile(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    connect_defect: Defect,
) -> Option<RecoveryAction> {
    let iface_id = match connect_defect {
        Defect::Iface(IfaceFailure::ConnectionFailure { iface_id }) => iface_id,
        other => {
            warn!("Assessing invalid defect type for connection failure recovery: {:?}", other);
            return None;
        }
    };

    thresholded_iface_destruction_and_phy_reset(
        phy_id,
        iface_id,
        defect_history,
        recovery_history,
        connect_defect,
        CONNECT_FAILURE_RECOVERY_THRESHOLD,
    )
}

fn thresholded_ap_start_failure_recovery_profile(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    ap_start_defect: Defect,
) -> Option<RecoveryAction> {
    match ap_start_defect {
        Defect::Iface(IfaceFailure::ApStartFailure { .. }) => thresholded_phy_reset(
            phy_id,
            defect_history,
            recovery_history,
            ap_start_defect,
            AP_START_FAILURE_RECOVERY_THRESHOLD,
        ),
        other => {
            warn!("Assessing invalid defect type for AP start failure recovery: {:?}", other);
            None
        }
    }
}

fn thresholded_create_iface_failure_recovery_profile(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    create_iface_defect: Defect,
) -> Option<RecoveryAction> {
    match create_iface_defect {
        Defect::Phy(PhyFailure::IfaceCreationFailure { .. }) => thresholded_phy_reset(
            phy_id,
            defect_history,
            recovery_history,
            create_iface_defect,
            CREATE_IFACE_FAILURE_RECOVERY_THRESHOLD,
        ),
        other => {
            warn!("Assessing invalid defect type for create iface failure recovery: {:?}", other);
            None
        }
    }
}

fn thresholded_destroy_iface_failure_recovery_profile(
    phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    destroy_iface_defect: Defect,
) -> Option<RecoveryAction> {
    match destroy_iface_defect {
        Defect::Phy(PhyFailure::IfaceDestructionFailure { .. }) => thresholded_phy_reset(
            phy_id,
            defect_history,
            recovery_history,
            destroy_iface_defect,
            DESTROY_IFACE_FAILURE_RECOVERY_THRESHOLD,
        ),
        other => {
            warn!("Assessing invalid defect type for destroy iface failure recovery: {:?}", other);
            None
        }
    }
}

fn thresholded_canceled_scan_recovery_profile(
    _phy_id: u16,
    defect_history: &mut EventHistory<Defect>,
    recovery_history: &mut EventHistory<RecoveryAction>,
    destroy_iface_defect: Defect,
) -> Option<RecoveryAction> {
    match destroy_iface_defect {
        Defect::Iface(IfaceFailure::CanceledScan { iface_id }) => thresholded_iface_destruction(
            iface_id,
            defect_history,
            recovery_history,
            destroy_iface_defect,
            CANCELED_SCAN_RECOVERY_THRESHOLD,
        ),
        other => {
            warn!("Assessing invalid defect type for canceled scan recovery: {:?}", other);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async::{MonotonicInstant, TestExecutor};

    use rand::RngExt as _;
    use test_case::test_case;

    #[fuchsia::test]
    fn test_recovery_action_equality() {
        let mut rng = rand::rng();
        assert_eq!(
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface {
                iface_id: rng.random::<u16>()
            }),
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface {
                iface_id: rng.random::<u16>()
            }),
        );
        assert_eq!(
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy {
                phy_id: rng.random::<u16>()
            }),
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy {
                phy_id: rng.random::<u16>()
            }),
        );
        assert_eq!(
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect {
                iface_id: rng.random::<u16>()
            }),
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::Disconnect {
                iface_id: rng.random::<u16>()
            }),
        );
        assert_eq!(
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp {
                iface_id: rng.random::<u16>()
            }),
            RecoveryAction::IfaceRecovery(IfaceRecoveryOperation::StopAp {
                iface_id: rng.random::<u16>()
            }),
        );
    }

    const PHY_ID: u16 = 123;
    const IFACE_ID: u16 = 456;

    // This test verifies that:
    // 1. No recovery is suggested until the failure recovery threshold is crossed.
    // 2. That interface destruction is suggested prior to PHY reset.
    // 3. Once interface destruction recovery is performed, it is not recommended until the time
    //    limit has elapsed.
    // 4. Once PHY reset has been suggested, it is not recommended again until the time limit has
    //    elapsed.
    fn test_thresholded_iface_destruction_and_phy_reset(
        exec: &TestExecutor,
        recovery_fn: RecoveryProfile,
        defect_to_log: Defect,
        defect_threshold: usize,
    ) {
        // Set the test time to start at time zero.
        let start_time = MonotonicInstant::from_nanos(0);
        exec.set_fake_time(start_time);

        // These are the potential recovery interventions that will be recommended.
        let destroy_iface_recommendation =
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id: IFACE_ID });
        let reset_phy_recommendation =
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: PHY_ID });

        // Retain defects and recovery actions for 48 hours.
        let mut defects = EventHistory::<Defect>::new(48 * 60 * 60);
        let mut recoveries = EventHistory::<RecoveryAction>::new(48 * 60 * 60);

        // Add failures until just under the threshold.
        for _ in 0..(defect_threshold - 1) {
            defects.add_event(defect_to_log);

            // Verify that there is no recovery recommended
            assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));
        }

        // Add one more failure and verify that a destroy iface was recommended.
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(destroy_iface_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );

        // Record the recovery action and then log another failure to verify that a PHY reset
        // is recommended.
        recoveries.add_event(destroy_iface_recommendation);
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(reset_phy_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );

        // Record the PHY reset and then advance the clock 11 hours and make sure that no recovery
        // is recommended.
        //
        // This is now 11 hours past the test start time.
        recoveries.add_event(reset_phy_recommendation);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(11)));
        defects.add_event(defect_to_log);
        assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));

        // Advance the clock another 2 hours to ensure that the time between iface destruction
        // recovery recommendations has elapsed.
        //
        // This is now 13 hours past the start of the test.
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(2)));
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(destroy_iface_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );

        // Record the destroy iface recovery action and advance the clock 10 more hours and make
        // sure that no recovery is recommended.
        //
        // This is now 23 hours past the start of the test.
        recoveries.add_event(destroy_iface_recommendation);
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(10)));
        defects.add_event(defect_to_log);
        assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));

        // Advance the clock another 2 hours to ensure that the time between PHY resets has elapsed.
        //
        // This is now 25 hours past the start of the test.
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(2)));
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(reset_phy_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );
    }

    // This test verifies that:
    // 1. No recovery is recommended until the threshold has been crossed.
    // 2. PHY reset is recommended once the threshold is crossed.
    // 3. PHY resets are only recommended once per 24-hour period.
    fn test_thresholded_phy_reset(
        exec: &TestExecutor,
        recovery_fn: RecoveryProfile,
        defect_to_log: Defect,
        defect_threshold: usize,
    ) {
        // Set the test time to start at time zero.
        let start_time = MonotonicInstant::from_nanos(0);
        exec.set_fake_time(start_time);

        // The PHY recovery intervention that is expected.
        let reset_phy_recommendation =
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::ResetPhy { phy_id: PHY_ID });

        // Retain defects and recovery actions for 48 hours.
        let mut defects = EventHistory::<Defect>::new(48 * 60 * 60);
        let mut recoveries = EventHistory::<RecoveryAction>::new(48 * 60 * 60);

        // Add failures until just under the threshold.
        for _ in 0..(defect_threshold - 1) {
            defects.add_event(defect_to_log);

            // Verify that there is no recovery recommended
            assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));
        }

        // Add one more failure and verify that a PHY reset was recommended.
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(reset_phy_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );
        recoveries.add_event(reset_phy_recommendation);

        // Add another defect and verify that no recovery is recommended.
        defects.add_event(defect_to_log);
        assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));

        // Advance the clock 23 hours, log another defect, and verify no recovery is recommended.
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(23)));
        defects.add_event(defect_to_log);
        assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));

        // Advance the clock another 2 hours to get beyond the 24 hour throttle and verify that
        // another occurrence of the defect results in a PHY reset recovery recommendation.
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(23)));
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(reset_phy_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );
    }

    fn test_thresholded_destroy_iface(
        exec: &TestExecutor,
        recovery_fn: RecoveryProfile,
        defect_to_log: Defect,
        defect_threshold: usize,
    ) {
        // Set the test time to start at time zero.
        let start_time = MonotonicInstant::from_nanos(0);
        exec.set_fake_time(start_time);

        // The iface destruction intervention that is expected.
        let destroy_iface_recommendation =
            RecoveryAction::PhyRecovery(PhyRecoveryOperation::DestroyIface { iface_id: IFACE_ID });

        // Retain defects and recovery actions for 48 hours.
        let mut defects = EventHistory::<Defect>::new(48 * 60 * 60);
        let mut recoveries = EventHistory::<RecoveryAction>::new(48 * 60 * 60);

        // Add failures until just under the threshold.
        for _ in 0..(defect_threshold - 1) {
            defects.add_event(defect_to_log);

            // Verify that there is no recovery recommended
            assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));
        }

        // Add one more failure and verify that a destroy iface was recommended.
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(destroy_iface_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );
        recoveries.add_event(destroy_iface_recommendation);

        // Add another defect and verify that no recovery is recommended.
        defects.add_event(defect_to_log);
        assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));

        // Advance the clock 11 hours, log another defect, and verify no recovery is recommended.
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(11)));
        defects.add_event(defect_to_log);
        assert_eq!(None, recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,));

        // Advance the clock another 2 hours to get beyond the 12 hour throttle and verify that
        // another occurrence of the defect results in a destroy iface recovery recommendation.
        exec.set_fake_time(MonotonicInstant::after(zx::MonotonicDuration::from_hours(2)));
        defects.add_event(defect_to_log);
        assert_eq!(
            Some(destroy_iface_recommendation),
            recovery_fn(PHY_ID, &mut defects, &mut recoveries, defect_to_log,)
        );
    }

    #[fuchsia::test]
    fn test_scan_failure_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Iface(IfaceFailure::FailedScan { iface_id: IFACE_ID });
        test_thresholded_iface_destruction_and_phy_reset(
            &exec,
            thresholded_scan_failure_recovery_profile,
            defect_to_log,
            SCAN_FAILURE_RECOVERY_THRESHOLD,
        );
    }

    #[fuchsia::test]
    fn test_empty_scan_results_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: IFACE_ID });
        test_thresholded_iface_destruction_and_phy_reset(
            &exec,
            thresholded_empty_scan_results_recovery_profile,
            defect_to_log,
            EMPTY_SCAN_RECOVERY_THRESHOLD,
        );
    }

    #[fuchsia::test]
    fn test_connect_failure_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: IFACE_ID });
        test_thresholded_iface_destruction_and_phy_reset(
            &exec,
            thresholded_connect_failure_recovery_profile,
            defect_to_log,
            CONNECT_FAILURE_RECOVERY_THRESHOLD,
        );
    }

    #[fuchsia::test]
    fn test_ap_start_failure_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Iface(IfaceFailure::ApStartFailure { iface_id: IFACE_ID });
        test_thresholded_phy_reset(
            &exec,
            thresholded_ap_start_failure_recovery_profile,
            defect_to_log,
            AP_START_FAILURE_RECOVERY_THRESHOLD,
        )
    }

    #[fuchsia::test]
    fn test_create_iface_failure_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: PHY_ID });
        test_thresholded_phy_reset(
            &exec,
            thresholded_create_iface_failure_recovery_profile,
            defect_to_log,
            CREATE_IFACE_FAILURE_RECOVERY_THRESHOLD,
        )
    }

    #[fuchsia::test]
    fn test_destroy_iface_failure_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: PHY_ID });
        test_thresholded_phy_reset(
            &exec,
            thresholded_destroy_iface_failure_recovery_profile,
            defect_to_log,
            DESTROY_IFACE_FAILURE_RECOVERY_THRESHOLD,
        )
    }

    #[fuchsia::test]
    fn test_canceled_scan_recovery() {
        let exec = TestExecutor::new_with_fake_time();
        let defect_to_log = Defect::Iface(IfaceFailure::CanceledScan { iface_id: IFACE_ID });
        test_thresholded_destroy_iface(
            &exec,
            thresholded_canceled_scan_recovery_profile,
            defect_to_log,
            CANCELED_SCAN_RECOVERY_THRESHOLD,
        )
    }

    #[test_case(
        Defect::Iface(IfaceFailure::FailedScan { iface_id: IFACE_ID }),
        SCAN_FAILURE_RECOVERY_THRESHOLD ;
        "scan failure threshold test"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::EmptyScanResults { iface_id: IFACE_ID }),
        EMPTY_SCAN_RECOVERY_THRESHOLD ;
        "empty scan threshold test"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::ConnectionFailure { iface_id: IFACE_ID }),
        CONNECT_FAILURE_RECOVERY_THRESHOLD ;
        "connection failure threshold test"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::ApStartFailure { iface_id: IFACE_ID }),
        AP_START_FAILURE_RECOVERY_THRESHOLD ;
        "start ap failure threshold test"
    )]
    #[test_case(
        Defect::Phy(PhyFailure::IfaceCreationFailure { phy_id: PHY_ID }),
        CREATE_IFACE_FAILURE_RECOVERY_THRESHOLD ;
        "create iface failure threshold test"
    )]
    #[test_case(
        Defect::Phy(PhyFailure::IfaceDestructionFailure { phy_id: PHY_ID }),
        DESTROY_IFACE_FAILURE_RECOVERY_THRESHOLD ;
        "destroy iface failure threshold test"
    )]
    #[test_case(
        Defect::Iface(IfaceFailure::CanceledScan { iface_id: IFACE_ID }),
        CANCELED_SCAN_RECOVERY_THRESHOLD ;
        "canceled scan threshold test"
    )]
    #[fuchsia::test(add_test_attr = false)]
    fn test_thresholded_recovery(defect: Defect, threshold: usize) {
        let exec = TestExecutor::new_with_fake_time();
        let profile = lookup_recovery_profile("thresholded_recovery");

        match defect {
            Defect::Iface(IfaceFailure::FailedScan { .. })
            | Defect::Iface(IfaceFailure::EmptyScanResults { .. })
            | Defect::Iface(IfaceFailure::ConnectionFailure { .. }) => {
                test_thresholded_iface_destruction_and_phy_reset(&exec, profile, defect, threshold)
            }
            Defect::Iface(IfaceFailure::ApStartFailure { .. })
            | Defect::Phy(PhyFailure::IfaceCreationFailure { .. })
            | Defect::Phy(PhyFailure::IfaceDestructionFailure { .. }) => {
                test_thresholded_phy_reset(&exec, profile, defect, threshold)
            }
            Defect::Iface(IfaceFailure::CanceledScan { .. }) => {
                test_thresholded_destroy_iface(&exec, profile, defect, threshold)
            }
            Defect::Iface(IfaceFailure::Timeout { .. }) => {
                // TODO(b/42066276): Determine thresholds and recovery options.
            }
        }
    }
}
