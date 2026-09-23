// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use wlan_legacy_metrics_registry as metrics;

pub fn convert_security_type(
    protection: &wlan_common::bss::Protection,
) -> metrics::SuccessfulConnectBreakdownBySecurityTypeMetricDimensionSecurityType {
    use metrics::SuccessfulConnectBreakdownBySecurityTypeMetricDimensionSecurityType::*;
    match protection {
        wlan_common::bss::Protection::Unknown => Unknown,
        wlan_common::bss::Protection::Open => Open,
        wlan_common::bss::Protection::Wep => Wep,
        wlan_common::bss::Protection::Wpa1 => Wpa1,
        wlan_common::bss::Protection::Wpa1Wpa2PersonalTkipOnly => Wpa1Wpa2PersonalTkipOnly,
        wlan_common::bss::Protection::Wpa2PersonalTkipOnly => Wpa2PersonalTkipOnly,
        wlan_common::bss::Protection::Wpa1Wpa2Personal => Wpa1Wpa2Personal,
        wlan_common::bss::Protection::Wpa2Personal => Wpa2Personal,
        wlan_common::bss::Protection::Wpa2Wpa3Personal => Wpa2Wpa3Personal,
        wlan_common::bss::Protection::Wpa3Personal => Wpa3Personal,
        wlan_common::bss::Protection::Wpa2Enterprise => Wpa2Enterprise,
        wlan_common::bss::Protection::Wpa3Enterprise => Wpa3Enterprise,
        wlan_common::bss::Protection::Owe => Owe,
        wlan_common::bss::Protection::OpenOweTransition => OpenOweTransition,
    }
}

pub fn convert_channel_band(
    band: fidl_fuchsia_wlan_ieee80211::WlanBand,
) -> metrics::SuccessfulConnectBreakdownByChannelBandMetricDimensionChannelBand {
    use metrics::SuccessfulConnectBreakdownByChannelBandMetricDimensionChannelBand::*;
    match band {
        fidl_fuchsia_wlan_ieee80211::WlanBand::FiveGhz => Band5Ghz,
        _ => Band2Dot4Ghz,
    }
}

pub fn convert_rssi_bucket(rssi: i8) -> metrics::ConnectivityWlanMetricDimensionRssiBucket {
    use metrics::ConnectivityWlanMetricDimensionRssiBucket::*;
    match rssi {
        -128..=-90 => From128To90,
        -89..=-86 => From89To86,
        -85..=-83 => From85To83,
        -82..=-80 => From82To80,
        -79..=-77 => From79To77,
        -76..=-74 => From76To74,
        -73..=-71 => From73To71,
        -70..=-66 => From70To66,
        -65..=-61 => From65To61,
        -60..=-51 => From60To51,
        -50..=-35 => From50To35,
        -34..=-28 => From34To28,
        -27..=-1 => From27To1,
        _ => _0,
    }
}

pub fn convert_snr_bucket(snr: i8) -> metrics::ConnectivityWlanMetricDimensionSnrBucket {
    use metrics::ConnectivityWlanMetricDimensionSnrBucket::*;
    match snr {
        1..=10 => From1To10,
        11..=15 => From11To15,
        16..=25 => From16To25,
        26..=40 => From26To40,
        41..=127 => MoreThan40,
        _ => _0,
    }
}

pub fn convert_is_owe_transition(
    is_owe_transition: bool,
) -> metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition {
    use metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition::*;
    if is_owe_transition { Yes } else { No }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test_case(true, metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition::Yes)]
    #[test_case(false, metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition::No)]
    fn test_convert_is_owe_transition(
        is_owe_transition: bool,
        expected: metrics::DailyConnectSuccessRateBreakdownByIsOweTransitionMetricDimensionIsOweTransition,
    ) {
        let actual = convert_is_owe_transition(is_owe_transition);
        assert_eq!(actual, expected);
    }
}
