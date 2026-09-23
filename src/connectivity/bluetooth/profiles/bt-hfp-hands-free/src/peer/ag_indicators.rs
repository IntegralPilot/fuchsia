// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Result, format_err};
use bt_hfp::call::indicators as call_indicators;
use fidl_fuchsia_bluetooth_hfp::SignalStrength;
use std::collections::HashMap;

use crate::impl_from_to_variant;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum AgIndicatorIndex {
    Call,
    CallSetup,
    CallHeld,
    ServiceAvailable,
    SignalStrength,
    Roaming,
    BatteryCharge,
    Vendor { name: String, min: i64, max: i64 },
}

impl AgIndicatorIndex {
    pub fn new(name: &str, min: i64, max: i64) -> Self {
        match name {
            "call" => Self::Call,
            "callsetup" => Self::CallSetup,
            "callheld" => Self::CallHeld,
            "service" => Self::ServiceAvailable,
            "signal" => Self::SignalStrength,
            "roam" => Self::Roaming,
            "battchg" => Self::BatteryCharge,
            other => Self::Vendor { name: other.to_string(), min, max },
        }
    }

    pub(crate) fn allowed_range(&self) -> std::ops::RangeInclusive<i64> {
        // These ranges are from HFP v1.8 4.32.2 section on AT+CIND.
        match self {
            Self::Call => 0..=1,
            Self::CallSetup => 0..=3,
            Self::CallHeld => 0..=2,
            Self::ServiceAvailable => 0..=1,
            Self::SignalStrength => 0..=5,
            Self::Roaming => 0..=1,
            Self::BatteryCharge => 0..=5,
            Self::Vendor { min, max, .. } => *min..=*max,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum CallIndicator {
    Call(call_indicators::Call),
    CallSetup(call_indicators::CallSetup),
    CallHeld(call_indicators::CallHeld),
}

impl_from_to_variant!(call_indicators::Call, CallIndicator, Call);
impl_from_to_variant!(call_indicators::CallSetup, CallIndicator, CallSetup);
impl_from_to_variant!(call_indicators::CallHeld, CallIndicator, CallHeld);

#[derive(Debug, PartialEq)]
pub enum NetworkInformationIndicator {
    ServiceAvailable(bool),
    SignalStrength(SignalStrength),
    Roaming(bool),
}

impl NetworkInformationIndicator {
    pub fn try_service_available_from_i64(value: i64) -> Result<Self> {
        match value {
            0 => Ok(Self::ServiceAvailable(false)),
            1 => Ok(Self::ServiceAvailable(true)),
            v => Err(format_err!("Unknown service indicator value: {v}")),
        }
    }

    pub fn try_signal_strength_from_i64(value: i64) -> Result<Self> {
        match value {
            0 => Ok(Self::SignalStrength(SignalStrength::None)),
            1 => Ok(Self::SignalStrength(SignalStrength::VeryLow)),
            2 => Ok(Self::SignalStrength(SignalStrength::Low)),
            3 => Ok(Self::SignalStrength(SignalStrength::Medium)),
            4 => Ok(Self::SignalStrength(SignalStrength::High)),
            5 => Ok(Self::SignalStrength(SignalStrength::VeryHigh)),
            v => Err(format_err!("Out of range signal strength value: {v}")),
        }
    }

    pub fn try_roaming_from_i64(value: i64) -> Result<Self> {
        match value {
            0 => Ok(Self::Roaming(false)),
            1 => Ok(Self::Roaming(true)),
            v => Err(format_err!("Unknown roaming indicator value: {v}")),
        }
    }
}

/// Battery charge 0-100.  This will be reported to fuchsia.bluetooth.power/Watcher/Watch, which
/// expects a percentage.
#[derive(Debug, PartialEq)]
pub struct BatteryChargeIndicator {
    pub percent: i64,
}

impl TryFrom<i64> for BatteryChargeIndicator {
    type Error = anyhow::Error;

    // A battchg indicator is between 0 and 5, inclusive. Convert this to a percentage.
    fn try_from(value: i64) -> Result<Self> {
        if value < 0 || value > 5 {
            Err(format_err!("Out of range battery charge value: {value}"))
        } else {
            let percent = value * 20;
            Ok(Self { percent })
        }
    }
}

/// Represents an arbitrary AG indicator that is not explicitly defined in the HFP specification.
/// See HFP v1.8 Section 4.2.1.3.
#[derive(Debug, PartialEq)]
pub struct VendorIndicator {
    pub name: String,
    pub value: i64,
}

impl VendorIndicator {
    pub fn try_new(name: String, min: i64, max: i64, value: i64) -> Result<Self> {
        if value < min || value > max {
            return Err(format_err!(
                "Value {value} is out of allowed range [{min}, {max}] for vendor indicator {name}"
            ));
        }
        Ok(Self { name, value })
    }
}

/// Typed AG Indicators, which represent the various +CIEV indicators the AG may send to the HF.
/// This is split into three variants which represent the different uses the HF has for these
/// indicators.
/// - Call indicators update the current calls in the Calls struct.
/// - TODO(https://fxbug.dev/131814) NetworkInformation indicators are returned to the client of the HFP
///   PeerHandler protocol via the WatchNetworkInformation hanging get call.
/// - TODO(https://fxbug.dev/131815) BatteryCharge is reported to the Power Reporting component and inspect.
#[derive(Debug, PartialEq)]
pub enum AgIndicator {
    Call(CallIndicator),
    NetworkInformation(NetworkInformationIndicator),
    BatteryCharge(BatteryChargeIndicator),
    Vendor(VendorIndicator),
}

impl_from_to_variant!(CallIndicator, AgIndicator, Call);
impl_from_to_variant!(NetworkInformationIndicator, AgIndicator, NetworkInformation);
impl_from_to_variant!(BatteryChargeIndicator, AgIndicator, BatteryCharge);
impl_from_to_variant!(VendorIndicator, AgIndicator, Vendor);

/// Keeps track of which indices are used for the indicators being received from an AG peer.
/// Map them to indicators from a `+CIEV` using [`AgIndicatorTranslator::translate`]
#[derive(Debug, PartialEq)]
pub struct AgIndicatorTranslator {
    indices: HashMap<i64, AgIndicatorIndex>,
}

impl AgIndicatorTranslator {
    pub fn new() -> Self {
        Self { indices: HashMap::new() }
    }

    pub fn set_index(&mut self, indicator: AgIndicatorIndex, index: i64) -> Result<()> {
        let result = self.indices.insert(index, indicator.clone());

        if let Some(old_indicator) = result {
            return Err(format_err!(
                "Duplicated AG indicators {:?} and {:?} specified for index {:}",
                indicator,
                old_indicator,
                index,
            ));
        }

        Ok(())
    }

    /// Translate a +CIEV indicator received from the AG to a typed AgIndicator using the index
    /// values previously retrieved from the AG via a +CIND.
    pub fn translate_indicator(&self, index: i64, value: i64) -> Result<AgIndicator> {
        let Some(ag_indicator_index) = self.indices.get(&index) else {
            return Err(format_err!(
                "Unknown indicator index {:}, current indices are {:?}.",
                index,
                self.indices
            ));
        };

        let indicator = match ag_indicator_index {
            AgIndicatorIndex::Call => {
                let call_indicator = call_indicators::Call::try_from(value)?;
                AgIndicator::Call(CallIndicator::Call(call_indicator))
            }
            AgIndicatorIndex::CallSetup => {
                let call_setup_indicator = call_indicators::CallSetup::try_from(value)?;
                AgIndicator::Call(CallIndicator::CallSetup(call_setup_indicator))
            }
            AgIndicatorIndex::CallHeld => {
                let call_held_indicator = call_indicators::CallHeld::try_from(value)?;
                AgIndicator::Call(CallIndicator::CallHeld(call_held_indicator))
            }
            AgIndicatorIndex::ServiceAvailable => {
                let service_available_indicator =
                    NetworkInformationIndicator::try_service_available_from_i64(value)?;
                AgIndicator::NetworkInformation(service_available_indicator)
            }
            AgIndicatorIndex::SignalStrength => {
                let signal_strength_indicator =
                    NetworkInformationIndicator::try_signal_strength_from_i64(value)?;
                AgIndicator::NetworkInformation(signal_strength_indicator)
            }
            AgIndicatorIndex::Roaming => {
                let roaming_indicator = NetworkInformationIndicator::try_roaming_from_i64(value)?;
                AgIndicator::NetworkInformation(roaming_indicator)
            }
            AgIndicatorIndex::BatteryCharge => {
                let battery_change_indicator = BatteryChargeIndicator::try_from(value)?;
                AgIndicator::BatteryCharge(battery_change_indicator)
            }
            AgIndicatorIndex::Vendor { name, min, max } => {
                let vendor = VendorIndicator::try_new(name.clone(), *min, *max, value)?;
                AgIndicator::Vendor(vendor)
            }
        };

        Ok(indicator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;

    #[fuchsia::test]
    fn indicators() {
        let mut translator = AgIndicatorTranslator::new();
        translator.set_index(AgIndicatorIndex::Call, 1).expect("Call");
        translator.set_index(AgIndicatorIndex::CallSetup, 2).expect("Call setup");
        translator.set_index(AgIndicatorIndex::CallHeld, 3).expect("Call held");
        translator.set_index(AgIndicatorIndex::ServiceAvailable, 4).expect("Sevice available");
        translator.set_index(AgIndicatorIndex::SignalStrength, 5).expect("Signal strength");
        translator.set_index(AgIndicatorIndex::Roaming, 6).expect("Roaming");
        translator.set_index(AgIndicatorIndex::BatteryCharge, 7).expect("Battery charge");

        // Call indicator
        let in_range_call_indicator =
            translator.translate_indicator(/* Call indicator */ 1, 1);
        assert_matches!(
            in_range_call_indicator,
            Ok(AgIndicator::Call(CallIndicator::Call(call_indicators::Call::Some)))
        );

        let out_of_range_call_indicator =
            translator.translate_indicator(/* Call indicator */ 1, 2);
        assert_matches!(out_of_range_call_indicator, Err(_));

        // Call Setup indicator
        let in_range_call_setup_indicator =
            translator.translate_indicator(/* Call setup indicator */ 2, 1);
        assert_matches!(
            in_range_call_setup_indicator,
            Ok(AgIndicator::Call(CallIndicator::CallSetup(call_indicators::CallSetup::Incoming)))
        );

        let out_of_range_call_setup_indicator =
            translator.translate_indicator(/* Calls setup indicator */ 2, 4);
        assert_matches!(out_of_range_call_setup_indicator, Err(_));

        // Call Held indicator
        let in_range_call_held_indicator =
            translator.translate_indicator(/* Call held indicator */ 3, 1);
        assert_matches!(
            in_range_call_held_indicator,
            Ok(AgIndicator::Call(CallIndicator::CallHeld(
                call_indicators::CallHeld::HeldAndActive
            )))
        );

        let out_of_range_call_held_indicator =
            translator.translate_indicator(/* Call held indicator */ 3, 4);
        assert_matches!(out_of_range_call_held_indicator, Err(_));

        // Service Available indicator
        let in_range_service_available_indicator =
            translator.translate_indicator(/* Service indicator */ 4, 1);
        assert_matches!(
            in_range_service_available_indicator,
            Ok(AgIndicator::NetworkInformation(NetworkInformationIndicator::ServiceAvailable(
                true
            )))
        );

        let out_of_range_service_available_indicator =
            translator.translate_indicator(/* Service indicator */ 4, 2);
        assert_matches!(out_of_range_service_available_indicator, Err(_));

        // Signal Strength indicator
        let in_range_signal_strength_indicator =
            translator.translate_indicator(/* Signal indicator */ 5, 1);
        assert_matches!(
            in_range_signal_strength_indicator,
            Ok(AgIndicator::NetworkInformation(NetworkInformationIndicator::SignalStrength(
                SignalStrength::VeryLow
            )))
        );

        let out_of_range_signal_strength_indicator =
            translator.translate_indicator(/* Signal indicator */ 5, 6);
        assert_matches!(out_of_range_signal_strength_indicator, Err(_));

        // Roam indicator
        let in_range_roaming_indicator =
            translator.translate_indicator(/* Roam indicator */ 6, 1);
        assert_matches!(
            in_range_roaming_indicator,
            Ok(AgIndicator::NetworkInformation(NetworkInformationIndicator::Roaming(true)))
        );

        let out_of_range_roaming_indicator =
            translator.translate_indicator(/* Roam indicator */ 6, 2);
        assert_matches!(out_of_range_roaming_indicator, Err(_));

        // Battery Charge indicator
        let in_range_battery_charge_indicator =
            translator.translate_indicator(/* Battchg indicator */ 7, 1);
        assert_matches!(
            in_range_battery_charge_indicator,
            Ok(AgIndicator::BatteryCharge(BatteryChargeIndicator { percent: 20 }))
        );

        let out_of_range_battery_charge_indicator =
            translator.translate_indicator(/* Battchg indicator */ 7, 6);
        assert_matches!(out_of_range_battery_charge_indicator, Err(_));

        // Vendor indicator range validation
        translator
            .set_index(AgIndicatorIndex::Vendor { name: "callfwd".to_string(), min: 0, max: 2 }, 8)
            .expect("Vendor");

        let in_range_vendor =
            translator.translate_indicator(/* Vendor indicator */ 8, 1).expect("valid range");
        assert_eq!(
            in_range_vendor,
            AgIndicator::Vendor(VendorIndicator { name: "callfwd".to_string(), value: 1 })
        );

        let out_of_range_vendor = translator.translate_indicator(/* Vendor indicator */ 8, 3);
        assert_matches!(out_of_range_vendor, Err(_));
    }

    #[fuchsia::test]
    fn unset_index() {
        let mut translator = AgIndicatorTranslator::new();
        translator.set_index(AgIndicatorIndex::Call, 1).expect("Call");

        // Call indicator exists
        let in_range_call_indicator =
            translator.translate_indicator(/* Call indicator */ 1, 1);
        assert_matches!(
            in_range_call_indicator,
            Ok(AgIndicator::Call(CallIndicator::Call(call_indicators::Call::Some)))
        );

        // Call Setup indicator does not exist
        let in_range_call_setup_indicator =
            translator.translate_indicator(/* Callsetup indicator */ 2, 1);
        assert_matches!(in_range_call_setup_indicator, Err(_));
    }

    #[fuchsia::test]
    fn reused_index_fails() {
        let mut translator = AgIndicatorTranslator::new();
        translator.set_index(AgIndicatorIndex::Call, 1).expect("Call");
        let result = translator.set_index(AgIndicatorIndex::Call, 1); // Reused index

        assert_matches!(result, Err(_));
    }

    #[fuchsia::test]
    fn multiple_vendor_indicators_same_name() {
        let mut translator = AgIndicatorTranslator::new();
        translator
            .set_index(AgIndicatorIndex::Vendor { name: "mycustom".to_string(), min: 0, max: 1 }, 8)
            .expect("Index 8");
        translator
            .set_index(AgIndicatorIndex::Vendor { name: "mycustom".to_string(), min: 0, max: 5 }, 9)
            .expect("Index 9");

        // Verify index 8 checks against range 0..=1
        assert_matches!(
            translator.translate_indicator(8, 1),
            Ok(AgIndicator::Vendor(VendorIndicator { value: 1, .. }))
        );
        assert_matches!(translator.translate_indicator(8, 4), Err(_));

        // Verify index 9 checks against range 0..=5
        assert_matches!(
            translator.translate_indicator(9, 4),
            Ok(AgIndicator::Vendor(VendorIndicator { value: 4, .. }))
        );

        // Value out of bounds.
        assert_matches!(translator.translate_indicator(9, 6), Err(_));
    }

    #[fuchsia::test]
    fn vendor_indicator_invalid_bounds() {
        // Max cannot be less than min.
        let res = VendorIndicator::try_new(
            "mycustom".to_string(),
            /* min */ 5,
            /* max */ 1,
            /* value */ 3,
        );
        assert_matches!(res, Err(_));
    }
}
