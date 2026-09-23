// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::{create_endpoints, create_proxy};
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use fuchsia_component::client::connect_to_protocol_at;
use std::pin::pin;
use wlan_hw_sim::event::action;
use wlan_hw_sim::*;

/// Test that we can connect to the policy service to discover a client interface when the
/// RegulatoryRegionWatcher cannot be reached.
#[fuchsia::test]
async fn run_without_regulatory_manager() {
    let mut helper = test_utils::TestHelper::begin_test(
        default_wlantap_config_client(),
        WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(false),
            with_policy: Some(true),
            ..Default::default()
        },
    )
    .await;
    let () = loop_until_iface_is_found(&mut helper).await;

    // Connect to the client policy service and get a client controller.
    let policy_provider =
        connect_to_protocol_at::<fidl_policy::ClientProviderMarker>(helper.test_ns_prefix())
            .expect("connecting to wlan policy");
    let (client_controller, server_end) = create_proxy();
    let (update_client_end, _update_server_end) = create_endpoints();
    let () =
        policy_provider.get_controller(server_end, update_client_end).expect("getting controller");

    // Issue a scan request to verify that the scan module can function in the absence of the
    // regulatory manager.
    let phy = helper.proxy();
    let fut = pin!(async move {
        let (scan_proxy, server_end) = create_proxy();
        client_controller.scan_for_networks(server_end).expect("requesting scan");
        loop {
            let result = scan_proxy.get_next().await.expect("getting scan results");
            let new_scan_results = result.expect("scanning failed");
            if new_scan_results.is_empty() {
                break;
            }
        }
        return;
    });
    helper
        .run_until_complete_or_timeout(
            *SCAN_RESPONSE_TEST_TIMEOUT,
            "receive a scan response",
            event::on_scan(action::send_scan_completion(&phy, 0)),
            fut,
        )
        .await;
}
