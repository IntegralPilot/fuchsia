// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use derivative::Derivative;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Platform configuration options for the input area.
#[derive(Debug, Deserialize, Serialize, PartialEq, JsonSchema, Derivative)]
#[derivative(Default)]
#[serde(default, deny_unknown_fields)]
pub struct TimekeeperConfig {
    /// The time to wait until retrying to sample the pull time source,
    /// expressed in seconds.
    #[derivative(Default(value = "300"))]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub back_off_time_between_pull_samples_sec: i64,
    /// The time to wait before sampling the time source for the first time,
    /// expressed in seconds.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub first_sampling_delay_sec: i64,
    /// If set, the device's real time clock is only ever read from, but
    /// not written to.
    #[derivative(Default(value = "\"https://clients3.google.com/generate_204\".into()"))]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub time_source_endpoint_url: String,
    /// If set, Timekeeper will serve test-only protocols from the library
    /// `fuchsia.time.test`.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub serve_test_protocols: bool,
    /// If set, the UTC clock will be started if we attempt to read the RTC,
    /// but the reading of the RTC is known invalid.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub utc_start_at_startup_when_invalid_rtc: bool,
    /// If set, Timekeeper will serve `fuchsia.time.alarms` and will connect
    /// to the appropriate hardware device to do so.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub serve_fuchsia_time_alarms: bool,
    /// If set, the hardware has a counter that is always on and operates even
    /// if the rest of the hardware system is in a low power state.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub always_on_counter: bool,
    /// If set, assembly should configure and route persistent storage to
    /// Timekeeper.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub use_persistent_storage: bool,
    /// If set, Timekeeper should serve the FIDL protocol that allows external
    /// time adjustment, `fuchsia.time.external/Adjust`.
    ///
    /// This is a security sensitive protocol, and very few assemblies are
    /// expected to have it turned on.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub serve_fuchsia_time_external_adjust: bool,
    /// Maximum absolute difference between proposed UTC reference and actual UTC
    /// reference, expressed in seconds, when the proposed UTC reference is
    /// in the "past" with respect of actual UTC reference.
    ///
    /// This is always expressed as a non-negative value.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub utc_max_allowed_delta_past_sec: u64,
    /// Maximum absolute difference between proposed UTC reference and actual UTC
    /// reference, expressed in seconds, when the proposed UTC reference is
    /// in the "future" with respect of actual UTC reference.
    ///
    /// This is always expressed as a non-negative value.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub utc_max_allowed_delta_future_sec: u64,
    /// If set, timekeeper will use the capabilities related to power management.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub power_topology_integration_enabled: bool,
    /// If set, Timekeeper will use connectivity information to gauge whether
    /// to sample external time sources or not.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub use_connectivity: bool,
    /// If we receive a UTC reference timestamp that is less than this amount of time away from
    /// backstop, we reject it.
    #[derivative(Default(value = "20"))]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub min_utc_reference_to_backstop_diff_minutes: u64,
    /// If we receive a UTC reference timestamp when boot time is less than
    /// this threshold, we reject it.
    #[derivative(Default(value = "5"))]
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub min_acceptable_boot_time_for_utc_update_minutes: u64,
    /// The policy for how to handle RTC readings that are in the past with respect
    /// to the current boot clock.
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub rtc_allow_setting_past_utc: RtcInitializationPolicy,
    /// The interval at which the RTC is updated from the UTC clock,
    #[serde(skip_serializing_if = "crate::common::is_default")]
    pub periodic_rtc_update: PeriodicRtcUpdate,
}

/// Used to turn on updating the UTC value recorded in the RTC with the most current UTC estimate
/// on a periodic basis.
///
/// A periodic update is strictly in addition to the "as needed" basis, which is always done.
/// Periodic update might be useful on systems where there is a weaker guarantee that RTC remains
/// recording usable value.
///
/// Normally we rely on RTC making independent progress and tracking UTC
/// as we record values to it. However, sometimes we want to make sure that any RTC drift with
/// respect to our most current UTC estimate is neutralized, at which point we may want to order
/// periodic updates. These updates come with a cost of more timekeeper activity, and hence more
/// aggressive power usage. As a result, the resolution of this value is 1 minute, and it is
/// recommended that the value is set to 10 minutes or more if a custom setting is needed.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PeriodicRtcUpdate {
    /// Never update rtc periodically.
    #[default]
    Disabled,
    /// Update every so many minutes.
    Minutes(u32),
}

impl PeriodicRtcUpdate {
    pub fn into_structured_config_value(self) -> u32 {
        match self {
            Self::Disabled => 0,
            Self::Minutes(v) => v,
        }
    }
}

/// The policy for how to handle RTC readings that are in the past with respect
/// to the current boot clock.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RtcInitializationPolicy {
    /// No change to existing behavior.
    #[default]
    Default,
    /// Apply the RTC reading even if it is in the past.
    ApplyMaybeStale,
}

impl From<RtcInitializationPolicy> for String {
    fn from(policy: RtcInitializationPolicy) -> Self {
        match policy {
            RtcInitializationPolicy::Default => "default".to_string(),
            RtcInitializationPolicy::ApplyMaybeStale => "apply_maybe_stale".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_default_serde() {
        let v: TimekeeperConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(v, Default::default());
    }

    #[test]
    fn test_default_serialization() {
        crate::common::tests::default_serialization_helper::<TimekeeperConfig>();
    }

    #[test]
    fn test_periodic_rtc_update_parsing() {
        let v: TimekeeperConfig =
            serde_json::from_str(r#"{"periodic_rtc_update": "disabled"}"#).unwrap();
        assert_eq!(v.periodic_rtc_update, PeriodicRtcUpdate::Disabled);

        let v: TimekeeperConfig =
            serde_json::from_str(r#"{"periodic_rtc_update": {"minutes": 10}}"#).unwrap();
        assert_eq!(v.periodic_rtc_update, PeriodicRtcUpdate::Minutes(10));
    }
}
