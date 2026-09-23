// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use at::BluetoothHFIndicator;
use at_commands as at;
use core::fmt::Debug;
use log::warn;

/// This mode's behavior is to forward unsolicited result codes directly per
/// 3GPP TS 27.007 version 6.8.0, Section 8.10.
/// This is the only supported mode in the Event Reporting Enabling AT command (AT+CMER).
/// Defined in HFP v1.8 Section 4.34.2.
pub const INDICATOR_REPORTING_MODE: i64 = 3;

/// Assigned numbers to supported HF Indicators according to the
/// Bluetooth SIG.
/// Defined in HFP v1.8 Section 4.36.1.1
pub const ENHANCED_SAFETY: u16 = 0x0001;
pub const BATTERY_LEVEL: u16 = 0x0002;

// A single indicator status + value.
#[derive(Clone, Copy, Debug)]
pub struct Indicator<T: Clone + Copy + Debug> {
    /// Whether this indicator is enabled or not.
    pub enabled: bool,
    /// The value of the indicator.
    // TODO(https://fxbug.dev/332390332): Remove or explain #[allow(dead_code)].
    #[allow(dead_code)]
    pub value: Option<T>,
}

impl<T: Clone + Copy + Debug> Default for Indicator<T> {
    fn default() -> Self {
        Self { enabled: true, value: None }
    }
}

/// The supported HF indicators and their enabled/disabled status & values.
/// The second bool determines whether the AG supports the indicator
/// Defined in HFP v1.8 Section 4.36
#[derive(Clone, Debug)]
pub struct HfIndicators {
    /// The Enhanced Safety HF indicator. There are only two potential values (enabled, disabled).
    pub enhanced_safety: (Indicator<bool>, bool),
    /// The Battery Level HF indicator. Can be any integer value between [0, 5].
    pub battery_level: (Indicator<u8>, bool),
}

impl Default for HfIndicators {
    fn default() -> Self {
        Self {
            enhanced_safety: (Indicator { enabled: false, value: None }, false),
            battery_level: (Indicator { enabled: false, value: None }, false),
        }
    }
}

impl HfIndicators {
    /// Enables the ability to enable/disable the HF indicator if the AG supports it.
    pub fn set_supported_indicators(&mut self, indicators: &Vec<BluetoothHFIndicator>) {
        for indicator in indicators {
            match indicator {
                BluetoothHFIndicator::BatteryLevel => {
                    self.battery_level.1 = true;
                }
                BluetoothHFIndicator::EnhancedSafety => {
                    self.enhanced_safety.1 = true;
                }
            }
        }
    }

    /// Checks to see if HF indicator is supported and then changes the state of it.
    pub fn change_indicator_state(&mut self, response: &at::Success) -> Result<(), Error> {
        match response {
            at::Success::BindStatus { anum: BluetoothHFIndicator::BatteryLevel, state }
                if self.battery_level.1 =>
            {
                self.battery_level.0.enabled = *state;
            }
            at::Success::BindStatus { anum: BluetoothHFIndicator::EnhancedSafety, state }
                if self.battery_level.1 =>
            {
                self.enhanced_safety.0.enabled = *state;
            }
            at::Success::BindStatus { anum, state: _ } => {
                warn!(
                    "Tried to enable unsupported HF Enhanced Safety indicator: {:?}, received {:?}",
                    anum, response
                )
            }
            _ => {
                return Err(format_err!(
                    "Received incorrect response. Recieved {:?} instead.",
                    response
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn indicators_become_enabled() {
        let supported_indicators =
            vec![at::BluetoothHFIndicator::BatteryLevel, at::BluetoothHFIndicator::EnhancedSafety];
        let mut hf_indicators = HfIndicators::default();
        // Indicator should be in not supported state.
        assert!(!hf_indicators.battery_level.1);
        assert!(!hf_indicators.enhanced_safety.1);
        hf_indicators.set_supported_indicators(&supported_indicators);
        assert!(hf_indicators.battery_level.1);
        assert!(hf_indicators.enhanced_safety.1);
    }

    #[fuchsia::test]
    fn unsupported_indicators_do_not_get_enabled() {
        let response1 =
            at::Success::BindStatus { anum: at::BluetoothHFIndicator::BatteryLevel, state: true };
        let response2 =
            at::Success::BindStatus { anum: at::BluetoothHFIndicator::EnhancedSafety, state: true };
        let mut hf_indicators = HfIndicators::default();
        // Indicator should be in not supported state.
        assert!(!hf_indicators.battery_level.1);
        assert!(!hf_indicators.enhanced_safety.1);
        // Indicators should default to disabled
        assert!(!hf_indicators.battery_level.0.enabled);
        assert!(!hf_indicators.enhanced_safety.0.enabled);
        assert_matches!(hf_indicators.change_indicator_state(&response1), Ok(_));
        assert_matches!(hf_indicators.change_indicator_state(&response2), Ok(_));
        // Indicator should remain disabled
        assert!(!hf_indicators.battery_level.0.enabled);
        assert!(!hf_indicators.enhanced_safety.0.enabled);
    }

    #[fuchsia::test]
    fn incorrect_response_returns_error_on_indicator_state() {
        let wrong_response = at::Success::TestResponse {};
        let mut hf_indicators = HfIndicators::default();
        assert_matches!(hf_indicators.change_indicator_state(&wrong_response), Err(_));
    }
}
