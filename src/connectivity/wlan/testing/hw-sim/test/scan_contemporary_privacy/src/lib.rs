// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use ieee80211::{Bssid, MacAddrBytes, Ssid};
use std::pin::pin;
use std::sync::LazyLock;
use wlan_common::bss::Protection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_hw_sim::event::action;
use wlan_hw_sim::*;

static BSS_WPA1: LazyLock<Bssid> =
    LazyLock::new(|| Bssid::from([0x62, 0x73, 0x73, 0x66, 0x6f, 0x6f]));
static BSS_WEP: LazyLock<Bssid> =
    LazyLock::new(|| Bssid::from([0x62, 0x73, 0x73, 0x66, 0x6f, 0x72]));
static BSS_MIXED: LazyLock<Bssid> =
    LazyLock::new(|| Bssid::from([0x62, 0x73, 0x73, 0x66, 0x6f, 0x7a]));
static SSID_WPA1: LazyLock<Ssid> = LazyLock::new(|| Ssid::try_from("wpa1___how_nice").unwrap());
static SSID_WEP: LazyLock<Ssid> = LazyLock::new(|| Ssid::try_from("wep_is_soooo_secure").unwrap());
static SSID_MIXED: LazyLock<Ssid> = LazyLock::new(|| Ssid::try_from("this_is_fine").unwrap());

/// Test a client cannot connect to a wep or wpa network when configured off.
#[fuchsia::test]
async fn scan_contemporary_privacy() {
    let mut helper = test_utils::TestHelper::begin_test(
        default_wlantap_config_client(),
        WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(true),
            with_policy: Some(true),
            ..Default::default()
        },
    )
    .await;
    let () = loop_until_iface_is_found(&mut helper).await;
    let phy = helper.proxy();

    // Create a client controller.
    let (client_controller, _update_stream) = init_client_controller(helper.test_ns_prefix()).await;

    let scan_result_list_fut = pin!(test_utils::policy_scan_for_networks(client_controller));
    let scan_result_list = helper
        .run_until_complete_or_timeout(
            *SCAN_RESPONSE_TEST_TIMEOUT,
            "receive a scan response",
            // Configure the scan event to return beacon frames corresponding to each `Beacon`
            // specified.
            event::on_scan(action::send_advertisements_and_scan_completion(
                &phy,
                [
                    Beacon {
                        channel: Channel::new(
                            1,
                            Bandwidth::Cbw20,
                            fidl_ieee80211::WlanBand::TwoGhz,
                        ),
                        bssid: *BSS_WPA1,
                        ssid: SSID_WPA1.clone(),
                        protection: Protection::Wpa1,
                        rssi_dbm: -30,
                    },
                    Beacon {
                        channel: Channel::new(
                            1,
                            Bandwidth::Cbw20,
                            fidl_ieee80211::WlanBand::TwoGhz,
                        ),
                        bssid: *BSS_WEP,
                        ssid: SSID_WEP.clone(),
                        protection: Protection::Wep,
                        rssi_dbm: -40,
                    },
                    Beacon {
                        channel: Channel::new(
                            1,
                            Bandwidth::Cbw20,
                            fidl_ieee80211::WlanBand::TwoGhz,
                        ),
                        bssid: *BSS_MIXED,
                        ssid: SSID_MIXED.clone(),
                        protection: Protection::Wpa1Wpa2Personal,
                        rssi_dbm: -50,
                    },
                ],
            )),
            scan_result_list_fut,
        )
        .await;

    let expected_scan_result_list = test_utils::sort_policy_scan_result_list(vec![
        fidl_policy::ScanResult {
            id: Some(fidl_policy::NetworkIdentifier {
                ssid: SSID_MIXED.to_vec(),
                type_: fidl_policy::SecurityType::Wpa2,
            }),
            entries: Some(vec![fidl_policy::Bss {
                bssid: Some(BSS_MIXED.to_array()),
                rssi: Some(-50),
                frequency: Some(2412),
                ..Default::default()
            }]),
            compatibility: Some(fidl_policy::Compatibility::Supported),
            ..Default::default()
        },
        fidl_policy::ScanResult {
            id: Some(fidl_policy::NetworkIdentifier {
                ssid: SSID_WEP.to_vec(),
                type_: fidl_policy::SecurityType::Wep,
            }),
            entries: Some(vec![fidl_policy::Bss {
                bssid: Some(BSS_WEP.to_array()),
                rssi: Some(-40),
                frequency: Some(2412),
                ..Default::default()
            }]),
            compatibility: Some(fidl_policy::Compatibility::DisallowedNotSupported),
            ..Default::default()
        },
        fidl_policy::ScanResult {
            id: Some(fidl_policy::NetworkIdentifier {
                ssid: SSID_WPA1.to_vec(),
                type_: fidl_policy::SecurityType::Wpa,
            }),
            entries: Some(vec![fidl_policy::Bss {
                bssid: Some(BSS_WPA1.to_array()),
                rssi: Some(-30),
                frequency: Some(2412),
                ..Default::default()
            }]),
            compatibility: Some(fidl_policy::Compatibility::DisallowedNotSupported),
            ..Default::default()
        },
    ]);

    // Compare one at a time for improved debuggability.
    assert_eq!(scan_result_list.len(), expected_scan_result_list.len());
    for i in 0..expected_scan_result_list.len() {
        assert_eq!(scan_result_list[i], expected_scan_result_list[i]);
    }
}
