// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_test_wlan_realm::WlanConfig;

use ieee80211::MacAddrBytes;
use wlan_hw_sim::{
    CLIENT_MAC_ADDR, default_wlantap_config_client, loop_until_iface_is_found, netdevice_helper,
    test_utils,
};

/// Test ethernet device is created successfully by verifying a ethernet client with specified
/// MAC address can be created successfully.
#[fuchsia::test]
async fn verify_ethernet() {
    let ctx = test_utils::TestRealmContext::new(WlanConfig {
        use_legacy_privacy: Some(false),
        with_regulatory_region: Some(true),
        with_policy: Some(true),
        ..Default::default()
    })
    .await;

    // Make sure there is no existing ethernet device.
    let client = netdevice_helper::create_client(
        ctx.devfs(),
        fidl_fuchsia_net::MacAddress { octets: CLIENT_MAC_ADDR.to_array() },
    )
    .await;
    assert!(client.is_none());

    // Create wlan_tap device which will in turn create ethernet device.
    let mut helper =
        test_utils::TestHelper::begin_test_with_context(ctx, default_wlantap_config_client()).await;
    let () = loop_until_iface_is_found(&mut helper).await;

    let mut retry = test_utils::RetryWithBackoff::infinite_with_max_interval(
        zx::MonotonicDuration::from_seconds(5),
    );
    loop {
        let client = netdevice_helper::create_client(
            helper.devfs(),
            fidl_fuchsia_net::MacAddress { octets: CLIENT_MAC_ADDR.to_array() },
        )
        .await;
        if client.is_some() {
            break;
        }
        retry.sleep_unless_after_deadline().await.unwrap_or_else(|_| {
            panic!("No netdevice client with mac_addr {:?} found in time", *CLIENT_MAC_ADDR)
        });
    }
}
