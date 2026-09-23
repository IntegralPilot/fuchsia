// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![cfg(test)]

use crate::client::roaming::lib::{PolicyRoamRequest, RoamReason, RoamingConnectionData};
use crate::client::types;
use crate::config_management::Credential;
use crate::util::pseudo_energy::EwmaSignalData;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_sme as fidl_sme;
use ieee80211::{Bssid, MacAddrBytes, Ssid};
use rand::distr::{Alphanumeric, SampleString};
use rand::{Rng as _, RngExt as _};
use std::collections::HashMap;
use wlan_common::bss::BssDescription;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::random_fidl_bss_description;
use wlan_common::scan::{Compatible, Incompatible};
use wlan_common::security::{SecurityAuthenticator, SecurityDescriptor, wep, wpa};

pub fn generate_ssid(ssid: &str) -> types::Ssid {
    types::Ssid::try_from(ssid).unwrap()
}

pub fn generate_security_type_detailed() -> types::SecurityTypeDetailed {
    types::SecurityTypeDetailed::from_primitive(rand::random_range(0..11)).unwrap()
}

/// Generate a random string of length 16
pub fn generate_string() -> String {
    Alphanumeric.sample_string(&mut rand::rng(), 16)
}

pub fn generate_random_channel() -> Channel {
    let mut rng = rand::rng();
    let band = if rng.random::<bool>() {
        fidl_ieee80211::WlanBand::TwoGhz
    } else {
        fidl_ieee80211::WlanBand::FiveGhz
    };
    let channel = match band {
        fidl_ieee80211::WlanBand::TwoGhz => rng.random_range(1..=14),
        fidl_ieee80211::WlanBand::FiveGhz => rng.random_range(32..=177),
        fidl_ieee80211::WlanBandUnknown!() => 36,
    };
    generate_channel(channel, band)
}

pub fn generate_channel(channel: u8, band: fidl_ieee80211::WlanBand) -> Channel {
    let mut rng = rand::rng();
    let cbw = match rng.random_range(0..5) {
        0 => Bandwidth::Cbw20,
        1 => Bandwidth::Cbw40,
        2 => Bandwidth::Cbw40Below,
        3 => Bandwidth::Cbw80,
        4 => Bandwidth::Cbw160,
        5 => Bandwidth::Cbw80P80 { vht_secondary_80_channel: rng.random::<u8>() },
        _ => panic!(),
    };
    Channel::new(channel, cbw, band)
}

pub fn generate_random_sme_scan_result() -> fidl_sme::ScanResult {
    let mut rng = rand::rng();
    fidl_sme::ScanResult {
        compatibility: match rng.random_range(0..4) {
            0 => fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_internal::Protocol::Open],
            }),
            1 => fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![fidl_internal::Protocol::Wpa2Personal],
            }),
            2 => fidl_sme::Compatibility::Compatible(fidl_sme::Compatible {
                mutual_security_protocols: vec![
                    fidl_internal::Protocol::Wpa2Personal,
                    fidl_internal::Protocol::Wpa3Personal,
                ],
            }),
            _ => fidl_sme::Compatibility::Incompatible(fidl_sme::Incompatible {
                description: String::from("unknown"),
                disjoint_security_protocols: None,
            }),
        },
        timestamp_nanos: rng.random(),
        bss_description: random_fidl_bss_description!(),
    }
}

pub fn generate_random_bssid() -> types::Bssid {
    Bssid::from(rand::random::<[u8; 6]>())
}

pub fn generate_random_bss() -> types::Bss {
    let mut rng = rand::rng();
    let bssid = generate_random_bssid();
    let rssi = rng.random_range(-100..20);
    let channel = generate_random_channel();
    let timestamp = zx::MonotonicInstant::from_nanos(rng.random());
    let snr_db = rng.random_range(-20..50);

    types::Bss {
        bssid,
        signal: types::Signal { rssi_dbm: rssi, snr_db },
        channel,
        timestamp,
        observation: if rng.random::<bool>() {
            types::ScanObservation::Passive
        } else {
            types::ScanObservation::Active
        },
        compatibility: match rng.random_range(0..4) {
            0 => Compatible::expect_ok([SecurityDescriptor::OPEN]),
            1 => Compatible::expect_ok([SecurityDescriptor::WPA2_PERSONAL]),
            2 => Compatible::expect_ok([
                SecurityDescriptor::WPA2_PERSONAL,
                SecurityDescriptor::WPA3_PERSONAL,
            ]),
            _ => Incompatible::unknown(),
        },
        bss_description: random_fidl_bss_description!(
            bssid: bssid.to_array(),
            rssi_dbm: rssi,
            channel: channel,
            snr_db: snr_db,
        )
        .into(),
    }
}

pub fn generate_random_bss_with_compatibility() -> types::Bss {
    types::Bss {
        compatibility: match rand::random_range(0..3) {
            0 => Compatible::expect_ok([SecurityDescriptor::OPEN]),
            1 => Compatible::expect_ok([SecurityDescriptor::WPA2_PERSONAL]),
            2 => Compatible::expect_ok([
                SecurityDescriptor::WPA2_PERSONAL,
                SecurityDescriptor::WPA3_PERSONAL,
            ]),
            _ => panic!(),
        },
        ..generate_random_bss()
    }
}

pub fn generate_random_ap_state() -> types::ApState {
    let bss_desc = BssDescription::try_from(random_fidl_bss_description!()).unwrap();
    types::ApState::from(bss_desc)
}

pub fn generate_random_saved_network_data() -> types::InternalSavedNetworkData {
    types::InternalSavedNetworkData {
        has_ever_connected: rand::random(),
        recent_failures: Vec::new(),
        past_connections: HashMap::new(),
    }
}

pub fn generate_random_scan_result() -> types::ScanResult {
    let mut rng = rand::rng();
    let ssid = Ssid::try_from(format!("scan result rand {}", rng.random::<i32>()))
        .expect("Failed to create random SSID from String");
    types::ScanResult {
        ssid,
        security_type_detailed: types::SecurityTypeDetailed::Wpa1,
        entries: vec![generate_random_bss(), generate_random_bss()],
        compatibility: match rng.random_range(0..2) {
            0 => types::Compatibility::Supported,
            1 => types::Compatibility::DisallowedNotSupported,
            2 => types::Compatibility::DisallowedInsecure,
            _ => panic!(),
        },
    }
}

pub fn generate_random_connect_reason() -> types::ConnectReason {
    match rand::random_range(0..6) {
        0 => types::ConnectReason::RetryAfterDisconnectDetected,
        1 => types::ConnectReason::RetryAfterFailedConnectAttempt,
        2 => types::ConnectReason::FidlConnectRequest,
        3 => types::ConnectReason::ProactiveNetworkSwitch,
        4 => types::ConnectReason::RegulatoryChangeReconnect,
        5 => types::ConnectReason::IdleInterfaceAutoconnect,
        6 => types::ConnectReason::NewSavedNetworkAutoconnect,
        _ => panic!(),
    }
}

pub fn generate_disconnect_info(is_sme_reconnecting: bool) -> fidl_sme::DisconnectInfo {
    fidl_sme::DisconnectInfo {
        is_sme_reconnecting,
        disconnect_source: match rand::random_range(0..2) {
            0 => fidl_sme::DisconnectSource::Ap(generate_random_disconnect_cause()),
            1 => fidl_sme::DisconnectSource::User(generate_random_user_disconnect_reason()),
            2 => fidl_sme::DisconnectSource::Mlme(generate_random_disconnect_cause()),
            _ => panic!(),
        },
    }
}

pub fn generate_random_user_disconnect_reason() -> fidl_sme::UserDisconnectReason {
    match rand::random_range(0..14) {
        0 => fidl_sme::UserDisconnectReason::Unknown,
        1 => fidl_sme::UserDisconnectReason::FailedToConnect,
        2 => fidl_sme::UserDisconnectReason::FidlConnectRequest,
        3 => fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest,
        4 => fidl_sme::UserDisconnectReason::ProactiveNetworkSwitch,
        5 => fidl_sme::UserDisconnectReason::DisconnectDetectedFromSme,
        6 => fidl_sme::UserDisconnectReason::RegulatoryRegionChange,
        7 => fidl_sme::UserDisconnectReason::Startup,
        8 => fidl_sme::UserDisconnectReason::NetworkUnsaved,
        9 => fidl_sme::UserDisconnectReason::NetworkConfigUpdated,
        10 => fidl_sme::UserDisconnectReason::WlanstackUnitTesting,
        11 => fidl_sme::UserDisconnectReason::WlanSmeUnitTesting,
        12 => fidl_sme::UserDisconnectReason::WlanServiceUtilTesting,
        13 => fidl_sme::UserDisconnectReason::WlanDevTool,
        _ => panic!(),
    }
}

pub fn generate_random_disconnect_cause() -> fidl_sme::DisconnectCause {
    fidl_sme::DisconnectCause {
        reason_code: generate_random_reason_code(),
        mlme_event_name: generate_random_disconnect_mlme_event_name(),
    }
}

pub fn generate_random_reason_code() -> fidl_ieee80211::ReasonCode {
    // This is just a random subset from the first few reason codes
    match rand::random_range(0..10) {
        0 => fidl_ieee80211::ReasonCode::UnspecifiedReason,
        1 => fidl_ieee80211::ReasonCode::InvalidAuthentication,
        2 => fidl_ieee80211::ReasonCode::LeavingNetworkDeauth,
        3 => fidl_ieee80211::ReasonCode::ReasonInactivity,
        4 => fidl_ieee80211::ReasonCode::NoMoreStas,
        5 => fidl_ieee80211::ReasonCode::InvalidClass2Frame,
        6 => fidl_ieee80211::ReasonCode::InvalidClass3Frame,
        7 => fidl_ieee80211::ReasonCode::LeavingNetworkDisassoc,
        8 => fidl_ieee80211::ReasonCode::NotAuthenticated,
        9 => fidl_ieee80211::ReasonCode::UnacceptablePowerCapability,
        _ => panic!(),
    }
}

pub fn generate_random_disconnect_mlme_event_name() -> fidl_sme::DisconnectMlmeEventName {
    match rand::random_range(0..2) {
        0 => fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
        1 => fidl_sme::DisconnectMlmeEventName::DisassociateIndication,
        _ => panic!(),
    }
}

pub fn generate_random_fidl_network_config() -> fidl_policy::NetworkConfig {
    let ssid = format!("random SSID {}", rand::random::<i32>());

    generate_random_fidl_network_config_with_ssid(&ssid)
}

pub fn generate_random_fidl_network_config_with_ssid(ssid: &str) -> fidl_policy::NetworkConfig {
    let credential_bytes = format!("rand pass {}", rand::random::<i32>()).into_bytes();

    fidl_policy::NetworkConfig {
        id: Some(fidl_policy::NetworkIdentifier {
            ssid: ssid.to_string().into_bytes(),
            type_: fidl_policy::SecurityType::Wpa2,
        }),
        credential: Some(fidl_policy::Credential::Password(credential_bytes)),
        ..Default::default()
    }
}

/// Generate a WPA2 network identifier with an SSID of length 2 to 32.
pub fn generate_random_network_identifier() -> types::NetworkIdentifier {
    let mut rng = rand::rng();
    let mut ssid = vec![0; rng.random_range(2..33)];
    rng.fill_bytes(&mut ssid);
    types::NetworkIdentifier {
        ssid: types::Ssid::from_bytes_unchecked(ssid),
        security_type: types::SecurityType::Wpa2,
    }
}

/// Generate a password of 8 to 64 random bytes.
pub fn generate_random_password() -> Credential {
    let mut rng = rand::rng();
    let password =
        vec![0; rng.random_range(8..64)].into_iter().map(|_| rng.random_range(0..128)).collect();
    Credential::Password(password)
}

pub fn generate_random_authenticator() -> SecurityAuthenticator {
    match rand::random_range(0..5) {
        0 => SecurityAuthenticator::Open,
        1 => {
            SecurityAuthenticator::Wep(wep::WepAuthenticator { key: wep::WepKey::Wep40(*b"five0") })
        }
        2 => SecurityAuthenticator::Wpa(wpa::WpaAuthenticator::Wpa1 {
            credentials: wpa::Wpa1Credentials::Passphrase(
                wpa::credential::Passphrase::try_from("password").unwrap(),
            ),
        }),
        3 => SecurityAuthenticator::Wpa(wpa::WpaAuthenticator::Wpa2 {
            cipher: None,
            authentication: wpa::Wpa2PersonalCredentials::Passphrase(
                wpa::credential::Passphrase::try_from("password").unwrap(),
            )
            .into(),
        }),
        4 => SecurityAuthenticator::Wpa(wpa::WpaAuthenticator::Wpa3 {
            cipher: None,
            authentication: wpa::Wpa3PersonalCredentials::Passphrase(
                wpa::credential::Passphrase::try_from("password").unwrap(),
            )
            .into(),
        }),
        _ => panic!(),
    }
}

pub fn generate_random_scanned_candidate() -> types::ScannedCandidate {
    let random_config = generate_random_fidl_network_config();
    types::ScannedCandidate {
        network: random_config.id.unwrap().clone().into(),
        security_type_detailed: generate_security_type_detailed(),
        credential: Credential::try_from(random_config.credential.unwrap().clone()).unwrap(),
        bss: generate_random_bss_with_compatibility(),
        network_has_multiple_bss: rand::random(),
        authenticator: generate_random_authenticator(),
        saved_network_info: generate_random_saved_network_data(),
    }
}

pub fn generate_connect_selection() -> types::ConnectSelection {
    types::ConnectSelection {
        target: generate_random_scanned_candidate(),
        reason: generate_random_connect_reason(),
    }
}

pub fn generate_random_signal() -> types::Signal {
    let mut rng = rand::rng();
    types::Signal { rssi_dbm: rng.random_range(-80..-20), snr_db: rng.random_range(0..80) }
}

pub fn generate_random_ewma_signal_data() -> EwmaSignalData {
    let mut rng = rand::rng();
    EwmaSignalData::new(
        rng.random_range(-80..-20),
        rng.random_range(0..80),
        rng.random_range(0..10) as usize,
    )
}

pub fn generate_random_roam_reason() -> RoamReason {
    match rand::random_range(0..1) {
        0 => RoamReason::RssiBelowThreshold,
        1 => RoamReason::SnrBelowThreshold,
        _ => panic!(),
    }
}

pub fn generate_random_roaming_connection_data() -> RoamingConnectionData {
    RoamingConnectionData::new(
        generate_random_ap_state(),
        generate_random_network_identifier(),
        generate_random_password(),
        generate_random_ewma_signal_data(),
    )
}

pub fn generate_policy_roam_request(bssid: types::Bssid) -> PolicyRoamRequest {
    PolicyRoamRequest {
        candidate: types::ScannedCandidate {
            bss: types::Bss { bssid, ..generate_random_bss() },
            ..generate_random_scanned_candidate()
        },
        reasons: vec![generate_random_roam_reason()],
    }
}
