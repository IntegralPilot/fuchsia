// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use ieee80211::{Bssid, Ssid};
use log::info;
use wlan_common::bss::Protection;
use wlan_hw_sim::*;

/// Test connections against all modern bool (non-WEP/WPA1) BSS protection types.
#[fuchsia::test]
async fn connect_to_modern_wpa_network() {
    let bss = Bssid::from(*b"wpaok!");
    // ssid, password, and psk are from "test case 2" of IEEE Std 802.11-2020 Annex J.4.2
    let ssid = Ssid::try_from("ThisIsASSID").unwrap();
    let password = Some("ThisIsAPassword");
    let psk = Some("0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af");

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

    let combinations = vec![
        // TODO(https://fxbug.dev/42052350): allow simulation for tkip
        // (Protection::Wpa1Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa, password),
        // (Protection::Wpa1Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa, psk),
        // (Protection::Wpa1Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa2, password),
        // (Protection::Wpa1Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa2, psk),
        // (Protection::Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa, password),
        // (Protection::Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa, psk),
        // (Protection::Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa2, password),
        // (Protection::Wpa2PersonalTkipOnly, fidl_policy::SecurityType::Wpa2, psk),
        (Protection::Wpa1Wpa2Personal, fidl_policy::SecurityType::Wpa, password),
        (Protection::Wpa1Wpa2Personal, fidl_policy::SecurityType::Wpa, psk),
        (Protection::Wpa1Wpa2Personal, fidl_policy::SecurityType::Wpa2, password),
        (Protection::Wpa1Wpa2Personal, fidl_policy::SecurityType::Wpa2, psk),
        (Protection::Wpa2Personal, fidl_policy::SecurityType::Wpa, password),
        (Protection::Wpa2Personal, fidl_policy::SecurityType::Wpa, psk),
        (Protection::Wpa2Personal, fidl_policy::SecurityType::Wpa2, password),
        (Protection::Wpa2Personal, fidl_policy::SecurityType::Wpa2, psk),
        // TODO(https://fxbug.dev/42166758): reenable credential upgrading
        // (Protection::Wpa2Wpa3Personal, fidl_policy::SecurityType::Wpa, password),
        (Protection::Wpa2Wpa3Personal, fidl_policy::SecurityType::Wpa2, password),
        (Protection::Wpa2Wpa3Personal, fidl_policy::SecurityType::Wpa3, password),
        // TODO(https://fxbug.dev/42166758): reenable credential upgrading
        // (Protection::Wpa3Personal, fidl_policy::SecurityType::Wpa, password),
        (Protection::Wpa3Personal, fidl_policy::SecurityType::Wpa2, password),
        (Protection::Wpa3Personal, fidl_policy::SecurityType::Wpa3, password),
    ];

    let test_ns_prefix = helper.test_ns_prefix().to_string();
    for (bss_protection, policy_security_type, credential) in combinations {
        info!(
            "Starting connection test for {} with Policy security {:?} and credential {:?}",
            bss_protection, policy_security_type, credential
        );

        let () = connect_or_timeout(
            &mut helper,
            zx::MonotonicDuration::from_seconds(30),
            &ssid,
            &bss,
            &bss_protection,
            credential,
            policy_security_type,
        )
        .await;

        // Remove the network and await disconnection
        let (client_controller, mut client_state_update_stream) =
            wlan_hw_sim::init_client_controller(&test_ns_prefix).await;
        forget_network(&client_controller, &ssid, policy_security_type).await;
        let id =
            fidl_policy::NetworkIdentifier { ssid: ssid.to_vec(), type_: policy_security_type };
        wait_until_client_state(&mut client_state_update_stream, |update| {
            has_id_and_state(update.clone(), &id, fidl_policy::ConnectionState::Disconnected)
                || update.networks.expect("networks unexpectedly unpopulated").is_empty()
        })
        .await;
    }
}
