// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use ieee80211::Bssid;
use wlan_common::bss::Protection;
use wlan_hw_sim::*;

/// Test a client successfully connects to a network protected by WPA1-PSK.
#[fuchsia::test]
async fn connect_to_wpa1_network() {
    let bss = Bssid::from(*b"wpa1ok");

    let mut helper = test_utils::TestHelper::begin_test(
        default_wlantap_config_client(),
        WlanConfig {
            use_legacy_privacy: Some(true),
            with_regulatory_region: Some(true),
            with_policy: Some(true),
            ..Default::default()
        },
    )
    .await;
    let () = loop_until_iface_is_found(&mut helper).await;

    let () = connect_or_timeout(
        &mut helper,
        zx::MonotonicDuration::from_seconds(30),
        &AP_SSID,
        &bss,
        &Protection::Wpa1,
        Some("wpa1good"),
        fidl_policy::SecurityType::Wpa,
    )
    .await;
}
