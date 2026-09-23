// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::config_management::Credential;
use crate::client::types;
use crate::util::pseudo_energy::{EwmaSignalData, RssiVelocity};
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fuchsia_async as fasync;
use log::error;
use wlan_common::historical_list::{HistoricalList, Timestamped};
use wlan_common::sequestered::Sequestered;

pub const ROAMING_CHANNEL_BUFFER_SIZE: usize = 100;
/// This is how many past roam events will be remembered for limiting roams per day. Each roam
/// monitor implementation decides how to limit roams per day, and this number must be greater
/// than or equal to all of them.
pub const NUM_PLATFORM_MAX_ROAMS_PER_DAY: usize = 5;
pub const TIMESPAN_TO_LIMIT_SCANS: zx::MonotonicDuration = zx::MonotonicDuration::from_hours(24);

// LINT.IfChange
#[derive(Clone, Copy, Debug)]
pub enum RoamingPolicy {
    Disabled,
    Enabled { profile: RoamingProfile, mode: RoamingMode },
}

#[derive(Clone, Copy, Debug)]
pub enum RoamingProfile {
    Stationary,
}
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub enum RoamingMode {
    MetricsOnly,
    #[default]
    CanRoam,
}

impl From<String> for RoamingPolicy {
    fn from(string: String) -> Self {
        match string.as_str() {
            "enabled_stationary_can_roam" => RoamingPolicy::Enabled {
                profile: RoamingProfile::Stationary,
                mode: RoamingMode::CanRoam,
            },
            "enabled_stationary_metrics_only" => RoamingPolicy::Enabled {
                profile: RoamingProfile::Stationary,
                mode: RoamingMode::MetricsOnly,
            },
            "disabled" => RoamingPolicy::Disabled,
            _ => {
                error!(
                    "Unknown roaming profile string ({}). Continuing with roaming disabled.",
                    string
                );
                RoamingPolicy::Disabled
            }
        }
    }
}
// LINT.ThenChange(//src/lib/assembly/config_schema/src/platform_config/connectivity_config.rs)

/// Data tracked about a connection used to make roaming decisions.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct RoamingConnectionData {
    pub ap_state: types::ApState,
    pub network_identifier: types::NetworkIdentifier,
    pub credential: Credential,
    pub signal_data: EwmaSignalData,
    pub rssi_velocity: RssiVelocity,
    pub previous_roam_scan_data: PreviousRoamScanData,
}
impl RoamingConnectionData {
    pub fn new(
        ap_state: types::ApState,
        network_identifier: types::NetworkIdentifier,
        credential: Credential,
        signal_data: EwmaSignalData,
    ) -> Self {
        Self {
            ap_state: ap_state.clone(),
            network_identifier,
            credential,
            signal_data,
            rssi_velocity: RssiVelocity::new(signal_data.ewma_rssi.get()),
            previous_roam_scan_data: PreviousRoamScanData::new(ap_state.tracked.signal.rssi_dbm),
        }
    }
}
// Metadata related to the previous roam scan event.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct PreviousRoamScanData {
    pub(crate) time: fasync::MonotonicInstant,
    /// This is the EWMA value, hence why it is an f64
    pub rssi: f64,
}
impl PreviousRoamScanData {
    pub fn new(rssi: impl Into<f64>) -> Self {
        Self { time: fasync::MonotonicInstant::now(), rssi: rssi.into() }
    }
}

// Data that could trigger roaming actions to occur. Roam monitor implementations
// MUST read all trigger data types from the channel, even if they ignore/drop it.
#[derive(Clone, Debug)]
pub enum RoamTriggerData {
    SignalReportInd(fidl_internal::SignalReportIndication),
}

// Actions that could be taken after handling new roam trigger data.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub enum RoamTriggerDataOutcome {
    Noop,
    RoamSearch {
        scan_type: fidl_common::ScanType,
        network_identifier: types::NetworkIdentifier,
        credential: Credential,
        current_security: types::SecurityTypeDetailed,
        reasons: Vec<RoamReason>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RoamReason {
    RssiBelowThreshold,
    SnrBelowThreshold,
}

#[derive(Clone)]
#[cfg_attr(test, derive(Debug, PartialEq))]
pub struct PolicyRoamRequest {
    pub candidate: types::ScannedCandidate,
    pub reasons: Vec<RoamReason>,
}
impl From<PolicyRoamRequest> for fidl_sme::RoamRequest {
    fn from(item: PolicyRoamRequest) -> Self {
        fidl_sme::RoamRequest {
            bss_description: Sequestered::release(item.candidate.bss.bss_description),
        }
    }
}

/// Only used for recording when the last roam attempts happened in order to limit their frequency.
#[derive(Clone)]
pub struct RoamEvent {
    time: fasync::MonotonicInstant,
}

impl RoamEvent {
    pub fn new_roam_now() -> Self {
        Self { time: fasync::MonotonicInstant::now() }
    }

    #[cfg(test)]
    pub fn new(time: fasync::MonotonicInstant) -> Self {
        Self { time }
    }
}

impl Timestamped for RoamEvent {
    fn time(&self) -> fasync::MonotonicInstant {
        self.time
    }
}

pub type PastRoamList = HistoricalList<RoamEvent, NUM_PLATFORM_MAX_ROAMS_PER_DAY>;
