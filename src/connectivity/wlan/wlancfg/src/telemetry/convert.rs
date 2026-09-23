// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::client::roaming::lib::RoamReason;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_sme as fidl_sme;
use log::error;
use wlan_common::bss::Protection as BssProtection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_metrics_registry as metrics;

pub fn convert_disconnect_source(
    source: &fidl_sme::DisconnectSource,
) -> metrics::ConnectivityWlanMetricDimensionDisconnectSource {
    use metrics::ConnectivityWlanMetricDimensionDisconnectSource::*;
    match source {
        fidl_sme::DisconnectSource::Ap(..) => Ap,
        fidl_sme::DisconnectSource::User(..) => User,
        fidl_sme::DisconnectSource::Mlme(..) => Mlme,
    }
}

pub fn convert_user_wait_time(
    duration: zx::MonotonicDuration,
) -> metrics::ConnectivityWlanMetricDimensionWaitTime {
    use metrics::ConnectivityWlanMetricDimensionWaitTime::*;
    match duration {
        x if x < zx::MonotonicDuration::from_seconds(1) => LessThan1Second,
        x if x < zx::MonotonicDuration::from_seconds(3) => LessThan3Seconds,
        x if x < zx::MonotonicDuration::from_seconds(5) => LessThan5Seconds,
        x if x < zx::MonotonicDuration::from_seconds(8) => LessThan8Seconds,
        x if x < zx::MonotonicDuration::from_seconds(15) => LessThan15Seconds,
        _ => AtLeast15Seconds,
    }
}

pub fn convert_is_multi_bss(
    multiple_bss_candidates: bool,
) -> metrics::SuccessfulConnectBreakdownByIsMultiBssMetricDimensionIsMultiBss {
    use metrics::SuccessfulConnectBreakdownByIsMultiBssMetricDimensionIsMultiBss::*;
    match multiple_bss_candidates {
        true => Yes,
        false => No,
    }
}

pub fn convert_security_type(
    protection: &BssProtection,
) -> metrics::SuccessfulConnectBreakdownBySecurityTypeMetricDimensionSecurityType {
    use metrics::SuccessfulConnectBreakdownBySecurityTypeMetricDimensionSecurityType::*;
    match protection {
        BssProtection::Unknown => Unknown,
        BssProtection::Open => Open,
        BssProtection::Wep => Wep,
        BssProtection::Wpa1 => Wpa1,
        BssProtection::Wpa1Wpa2PersonalTkipOnly => Wpa1Wpa2PersonalTkipOnly,
        BssProtection::Wpa2PersonalTkipOnly => Wpa2PersonalTkipOnly,
        BssProtection::Wpa1Wpa2Personal => Wpa1Wpa2Personal,
        BssProtection::Wpa2Personal => Wpa2Personal,
        BssProtection::Wpa2Wpa3Personal => Wpa2Wpa3Personal,
        BssProtection::Wpa3Personal => Wpa3Personal,
        BssProtection::Wpa2Enterprise => Wpa2Enterprise,
        BssProtection::Wpa3Enterprise => Wpa3Enterprise,
        BssProtection::Owe => Owe,
        BssProtection::OpenOweTransition => OpenOweTransition,
    }
}

pub fn convert_channel_band(
    band: fidl_ieee80211::WlanBand,
) -> metrics::SuccessfulConnectBreakdownByChannelBandMetricDimensionChannelBand {
    use metrics::SuccessfulConnectBreakdownByChannelBandMetricDimensionChannelBand::*;
    match band {
        fidl_ieee80211::WlanBand::FiveGhz => Band5Ghz,
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

pub fn convert_roam_reason_dimension(
    reason: RoamReason,
) -> metrics::PolicyRoamConnectedDurationBeforeRoamAttemptMetricDimensionReason {
    use metrics::PolicyRoamConnectedDurationBeforeRoamAttemptMetricDimensionReason::*;
    match reason {
        RoamReason::RssiBelowThreshold => RssiBelowThreshold,
        RoamReason::SnrBelowThreshold => SnrBelowThreshold,
    }
}

pub fn get_ghz_band_transition(
    origin_channel: &Channel,
    target_channel: &Channel,
) -> metrics::ConnectivityWlanMetricDimensionGhzBandTransition {
    let origin_is_2g = origin_channel.band == fidl_ieee80211::WlanBand::TwoGhz;
    let origin_is_5g = origin_channel.band == fidl_ieee80211::WlanBand::FiveGhz;
    let target_is_2g = target_channel.band == fidl_ieee80211::WlanBand::TwoGhz;
    let target_is_5g = target_channel.band == fidl_ieee80211::WlanBand::FiveGhz;

    match (origin_is_2g, origin_is_5g, target_is_2g, target_is_5g) {
        (true, false, true, false) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From2gTo2g
        }
        (true, false, false, true) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From2gTo5g
        }
        (true, false, false, false) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From2gTo6g
        }
        (false, true, true, false) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From5gTo2g
        }
        (false, true, false, true) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From5gTo5g
        }
        (false, true, false, false) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From5gTo6g
        }
        (false, false, true, false) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From6gTo2g
        }
        (false, false, false, true) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From6gTo5g
        }
        (false, false, false, false) => {
            metrics::ConnectivityWlanMetricDimensionGhzBandTransition::From6gTo6g
        }
        _ => panic!("Invalid channel band combination"),
    }
}

// Convert an RSSI delta (i8) into the bucket index for an RSSI delta based histogram, where bucket
// indexes run from 0 to 128, covering delta values -128 to 127, with step size of 2.
//
// bucket_index = (delta + 129) / 2
pub fn calculate_rssi_delta_bucket(delta: i8) -> i64 {
    let num_buckets: i64 = metrics::POLICY_ROAM_TRANSITION_RSSI_DELTA_BY_ROAM_REASON_FLEETWIDE_HISTOGRAM_INT_BUCKETS_NUM_BUCKETS.into();
    let step_size: i64 = metrics::POLICY_ROAM_TRANSITION_RSSI_DELTA_BY_ROAM_REASON_FLEETWIDE_HISTOGRAM_INT_BUCKETS_STEP_SIZE.into();
    let idx = delta as i64 + num_buckets;

    // "Euclidean" division floors towards -infinity (rather than toward zero).
    // By subtracting 1 before dividing and adding 1 after, we achieve ceiling division
    // that works across positive and negative numbers.
    // TODO(https://github.com/rust-lang/rust/issues/88581): Replace with `{integer}::div_ceil()`
    // when `int_roundings` is available.
    (idx - 1).div_euclid(step_size) + 1
}

pub fn convert_disconnect_info(
    info: &crate::telemetry::DisconnectInfo,
) -> wlan_telemetry::DisconnectInfo {
    wlan_telemetry::DisconnectInfo {
        iface_id: info.iface_id,
        connected_duration: zx::BootDuration::from_nanos(info.connected_duration.into_nanos()),
        is_sme_reconnecting: info.is_sme_reconnecting,
        disconnect_source: info.disconnect_source,
        original_bss_desc: Box::new(info.ap_state.original().clone()),
        current_rssi_dbm: info.ap_state.tracked.signal.rssi_dbm,
        current_snr_db: info.ap_state.tracked.signal.snr_db,
        current_channel: info.ap_state.tracked.channel,
    }
}

pub fn convert_client_connections_toggle(
    event: &crate::telemetry::TelemetryEvent,
) -> Option<wlan_telemetry::ClientConnectionsToggleEvent> {
    match event {
        crate::telemetry::TelemetryEvent::StartClientConnectionsRequest => {
            Some(wlan_telemetry::ClientConnectionsToggleEvent::Enabled)
        }
        crate::telemetry::TelemetryEvent::StopClientConnectionsRequest => {
            Some(wlan_telemetry::ClientConnectionsToggleEvent::Disabled)
        }
        _ => None,
    }
}

pub fn convert_to_wlan_telemetry_event(
    event: &crate::telemetry::TelemetryEvent,
) -> Option<wlan_telemetry::TelemetryEvent> {
    match event {
        crate::telemetry::TelemetryEvent::ConnectResult { result, ap_state, .. } => {
            Some(wlan_telemetry::TelemetryEvent::ConnectResult {
                result: result.code,
                bss: Box::new(ap_state.original().clone()),
                is_credential_rejected: result.is_credential_rejected,
                is_owe_transition: ap_state.original().protection()
                    == BssProtection::OpenOweTransition,
            })
        }
        crate::telemetry::TelemetryEvent::Disconnected { info: Some(info), .. } => {
            Some(wlan_telemetry::TelemetryEvent::Disconnect { info: convert_disconnect_info(info) })
        }
        crate::telemetry::TelemetryEvent::StartClientConnectionsRequest
        | crate::telemetry::TelemetryEvent::StopClientConnectionsRequest => {
            convert_client_connections_toggle(event).map(|toggle| {
                wlan_telemetry::TelemetryEvent::ClientConnectionsToggle { event: toggle }
            })
        }
        crate::telemetry::TelemetryEvent::IfaceCreationResult { role, result } => {
            match (role, result) {
                (fidl_common::WlanMacRole::Client, Ok(iface_id)) => {
                    Some(wlan_telemetry::TelemetryEvent::ClientIfaceCreated { iface_id: *iface_id })
                }
                (_, Err(())) => Some(wlan_telemetry::TelemetryEvent::IfaceCreationFailure),
                _ => None,
            }
        }
        crate::telemetry::TelemetryEvent::IfaceDestructionResult { role, result } => {
            match (role, result) {
                (fidl_common::WlanMacRole::Client, Ok(iface_id)) => {
                    Some(wlan_telemetry::TelemetryEvent::ClientIfaceDestroyed {
                        iface_id: *iface_id,
                    })
                }
                (_, Err(())) => Some(wlan_telemetry::TelemetryEvent::IfaceDestructionFailure),
                _ => None,
            }
        }
        crate::telemetry::TelemetryEvent::SmeTimeout { source } => {
            Some(wlan_telemetry::TelemetryEvent::SmeTimeout { source: *source })
        }
        crate::telemetry::TelemetryEvent::RecoveryEvent { .. } => {
            Some(wlan_telemetry::TelemetryEvent::RecoveryEvent)
        }
        crate::telemetry::TelemetryEvent::OnChannelSwitched { info } => {
            let bandwidth =
                match Bandwidth::from_fidl(info.bandwidth, info.vht_secondary_80_channel.number) {
                    Ok(bandwidth) => bandwidth,
                    Err(e) => {
                        error!("Invalid Bandwidth in ChannelSwitchInfo: {}", e);
                        Bandwidth::Cbw20
                    }
                };
            Some(wlan_telemetry::TelemetryEvent::ChannelSwitched {
                channel: Channel::new(
                    info.new_primary_channel.number,
                    bandwidth,
                    info.new_primary_channel.band,
                ),
            })
        }
        crate::telemetry::TelemetryEvent::SmeScanStart => {
            Some(wlan_telemetry::TelemetryEvent::ScanStart)
        }
        crate::telemetry::TelemetryEvent::SmeScanResult { result } => {
            Some(wlan_telemetry::TelemetryEvent::ScanResult { result: *result })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use test_case::test_case;
    use wlan_common::random_bss_description;

    #[fuchsia::test]
    fn test_calculate_rssi_delta_bucket() {
        // Lowest bucket encompasses i8::MIN, so hitting underflow bucket (idx: 0)
        // is not possible.
        assert_eq!(calculate_rssi_delta_bucket(i8::MIN), 1);

        // Linear buckets for deltas, with step size of 2
        assert_eq!(calculate_rssi_delta_bucket(-127), 1);
        assert_eq!(calculate_rssi_delta_bucket(0), 65);
        assert_eq!(calculate_rssi_delta_bucket(1), 65);
        assert_eq!(calculate_rssi_delta_bucket(i8::MAX), 128);
    }

    #[fuchsia::test]
    fn test_convert_client_connections_toggle() {
        assert_eq!(
            convert_client_connections_toggle(
                &crate::telemetry::TelemetryEvent::StartClientConnectionsRequest
            ),
            Some(wlan_telemetry::ClientConnectionsToggleEvent::Enabled)
        );
        assert_eq!(
            convert_client_connections_toggle(
                &crate::telemetry::TelemetryEvent::StopClientConnectionsRequest
            ),
            Some(wlan_telemetry::ClientConnectionsToggleEvent::Disabled)
        );
        assert_eq!(
            convert_client_connections_toggle(
                &crate::telemetry::TelemetryEvent::ClearEstablishConnectionStartTime
            ),
            None
        );
    }

    #[fuchsia::test]
    fn test_convert_disconnect_info() {
        let bss = random_bss_description!(Wpa2);
        let mut ap_state: crate::client::types::ApState = bss.clone().into();
        ap_state.tracked.signal.rssi_dbm = -55;
        ap_state.tracked.signal.snr_db = 32;
        let info = crate::telemetry::DisconnectInfo {
            iface_id: 1,
            connected_duration: zx::MonotonicDuration::from_seconds(120),
            is_sme_reconnecting: true,
            disconnect_source: fidl_sme::DisconnectSource::User(
                fidl_sme::UserDisconnectReason::FidlConnectRequest,
            ),
            previous_connect_reason: crate::client::types::ConnectReason::IdleInterfaceAutoconnect,
            ap_state,
            signals: wlan_common::historical_list::HistoricalList::new(),
        };

        let converted = convert_disconnect_info(&info);
        assert_eq!(converted.iface_id, 1);
        assert_eq!(converted.connected_duration, zx::BootDuration::from_seconds(120));
        assert!(converted.is_sme_reconnecting);
        assert_eq!(
            converted.disconnect_source,
            fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::FidlConnectRequest)
        );
        assert_eq!(converted.original_bss_desc.bssid, bss.bssid);
        assert_eq!(converted.current_rssi_dbm, -55);
        assert_eq!(converted.current_snr_db, 32);
        assert_eq!(converted.current_channel, bss.channel);
    }

    #[fuchsia::test]
    fn test_convert_connect_result() {
        let bss = random_bss_description!(Wpa2);
        let connect_result_event = crate::telemetry::TelemetryEvent::ConnectResult {
            iface_id: 0,
            policy_connect_reason: None,
            result: fidl_sme::ConnectResult {
                code: fidl_fuchsia_wlan_ieee80211::StatusCode::Success,
                is_credential_rejected: false,
                is_reconnect: false,
            },
            multiple_bss_candidates: false,
            ap_state: bss.clone().into(),
            network_is_likely_hidden: false,
        };

        let converted = convert_to_wlan_telemetry_event(&connect_result_event);
        match converted {
            Some(wlan_telemetry::TelemetryEvent::ConnectResult {
                result,
                bss: converted_bss,
                is_credential_rejected,
                ..
            }) => {
                assert_eq!(result, fidl_fuchsia_wlan_ieee80211::StatusCode::Success);
                assert_eq!(converted_bss.bssid, bss.bssid);
                assert!(!is_credential_rejected);
            }
            _ => panic!("Expected ConnectResult event"),
        }
    }

    #[fuchsia::test]
    fn test_convert_sme_timeout() {
        assert_matches!(
            convert_to_wlan_telemetry_event(&crate::telemetry::TelemetryEvent::SmeTimeout {
                source: wlan_telemetry::TimeoutSource::Scan,
            }),
            Some(wlan_telemetry::TelemetryEvent::SmeTimeout {
                source: wlan_telemetry::TimeoutSource::Scan,
            })
        );
    }

    #[fuchsia::test]
    fn test_convert_disconnected() {
        let bss = random_bss_description!(Wpa2);
        let disconnect_info = crate::telemetry::DisconnectInfo {
            iface_id: 42,
            connected_duration: zx::MonotonicDuration::from_seconds(120),
            is_sme_reconnecting: true,
            disconnect_source: fidl_sme::DisconnectSource::User(
                fidl_sme::UserDisconnectReason::FidlConnectRequest,
            ),
            previous_connect_reason: crate::client::types::ConnectReason::IdleInterfaceAutoconnect,
            ap_state: bss.clone().into(),
            signals: wlan_common::historical_list::HistoricalList::new(),
        };

        let converted_disconnect =
            convert_to_wlan_telemetry_event(&crate::telemetry::TelemetryEvent::Disconnected {
                track_subsequent_downtime: true,
                info: Some(disconnect_info),
            });
        match converted_disconnect {
            Some(wlan_telemetry::TelemetryEvent::Disconnect { info }) => {
                assert_eq!(info.iface_id, 42);
            }
            _ => panic!("Expected Disconnect event"),
        }
    }

    #[fuchsia::test]
    fn test_convert_recovery_event() {
        assert_matches!(
            convert_to_wlan_telemetry_event(&crate::telemetry::TelemetryEvent::RecoveryEvent {
                reason: crate::telemetry::RecoveryReason::Timeout(
                    crate::telemetry::TimeoutRecoveryMechanism::PhyReset,
                ),
            }),
            Some(wlan_telemetry::TelemetryEvent::RecoveryEvent)
        );
    }

    #[test_case(fidl_common::WlanMacRole::Client, Ok(42), Some(42); "client success")]
    #[test_case(fidl_common::WlanMacRole::Ap, Ok(42), None; "ap success returns none")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_convert_iface_creation_success(
        role: fidl_common::WlanMacRole,
        result: Result<u16, ()>,
        expected_iface_id: Option<u16>,
    ) {
        let event = crate::telemetry::TelemetryEvent::IfaceCreationResult { role, result };
        let converted = convert_to_wlan_telemetry_event(&event);
        match expected_iface_id {
            Some(iface_id) => {
                assert_matches!(
                    converted,
                    Some(wlan_telemetry::TelemetryEvent::ClientIfaceCreated { iface_id: id }) if id == iface_id
                );
            }
            None => assert_matches!(converted, None),
        }
    }

    #[test_case(fidl_common::WlanMacRole::Client; "client failure")]
    #[test_case(fidl_common::WlanMacRole::Ap; "ap failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_convert_iface_creation_failure(role: fidl_common::WlanMacRole) {
        let event = crate::telemetry::TelemetryEvent::IfaceCreationResult { role, result: Err(()) };
        assert_matches!(
            convert_to_wlan_telemetry_event(&event),
            Some(wlan_telemetry::TelemetryEvent::IfaceCreationFailure)
        );
    }

    #[test_case(fidl_common::WlanMacRole::Client, Ok(42), Some(42); "client success")]
    #[test_case(fidl_common::WlanMacRole::Ap, Ok(42), None; "ap success returns none")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_convert_iface_destruction_success(
        role: fidl_common::WlanMacRole,
        result: Result<u16, ()>,
        expected_iface_id: Option<u16>,
    ) {
        let event = crate::telemetry::TelemetryEvent::IfaceDestructionResult { role, result };
        let converted = convert_to_wlan_telemetry_event(&event);
        match expected_iface_id {
            Some(iface_id) => {
                assert_matches!(
                    converted,
                    Some(wlan_telemetry::TelemetryEvent::ClientIfaceDestroyed { iface_id: id }) if id == iface_id
                );
            }
            None => assert_matches!(converted, None),
        }
    }

    #[test_case(fidl_common::WlanMacRole::Client; "client failure")]
    #[test_case(fidl_common::WlanMacRole::Ap; "ap failure")]
    #[fuchsia::test(add_test_attr = false)]
    fn test_convert_iface_destruction_failure(role: fidl_common::WlanMacRole) {
        let event =
            crate::telemetry::TelemetryEvent::IfaceDestructionResult { role, result: Err(()) };
        assert_matches!(
            convert_to_wlan_telemetry_event(&event),
            Some(wlan_telemetry::TelemetryEvent::IfaceDestructionFailure)
        );
    }

    #[fuchsia::test]
    fn test_convert_on_channel_switched() {
        let event = crate::telemetry::TelemetryEvent::OnChannelSwitched {
            info: fidl_fuchsia_wlan_internal::ChannelSwitchInfo {
                new_primary_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::FiveGhz,
                    number: 157,
                },
                bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::FiveGhz,
                    number: 0,
                },
            },
        };
        assert_matches!(
            convert_to_wlan_telemetry_event(&event),
            Some(wlan_telemetry::TelemetryEvent::ChannelSwitched { channel }) => {
                assert_eq!(channel.primary, 157);
                assert_eq!(channel.band, fidl_ieee80211::WlanBand::FiveGhz);
                assert_eq!(channel.bandwidth, Bandwidth::Cbw20);
            }
        );
    }

    #[fuchsia::test]
    fn test_convert_sme_scan_start() {
        assert_matches!(
            convert_to_wlan_telemetry_event(&crate::telemetry::TelemetryEvent::SmeScanStart),
            Some(wlan_telemetry::TelemetryEvent::ScanStart)
        );
    }

    #[fuchsia::test]
    fn test_convert_sme_scan_result() {
        assert_matches!(
            convert_to_wlan_telemetry_event(&crate::telemetry::TelemetryEvent::SmeScanResult {
                result: wlan_telemetry::ScanResult::Complete { num_results: 5 }
            }),
            Some(wlan_telemetry::TelemetryEvent::ScanResult {
                result: wlan_telemetry::ScanResult::Complete { num_results: 5 }
            })
        );
    }

    #[fuchsia::test]
    fn test_convert_unhandled_event_returns_none() {
        assert_matches!(
            convert_to_wlan_telemetry_event(
                &crate::telemetry::TelemetryEvent::ClearEstablishConnectionStartTime
            ),
            None
        );
    }
}
