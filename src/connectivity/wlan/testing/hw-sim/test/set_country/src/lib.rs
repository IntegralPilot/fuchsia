// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_device_service::{
    DeviceMonitorMarker, DeviceMonitorProxy, SetCountryRequest,
};
use fidl_test_wlan_realm::WlanConfig;
use fuchsia_component::client::connect_to_protocol_at;

use futures::channel::oneshot;
use std::pin::pin;
use wlan_hw_sim::event::Handler;
use wlan_hw_sim::*;
use zx::sys::ZX_OK;

async fn set_country_and_await_match<'a>(
    receiver: oneshot::Receiver<()>,
    svc: &'a DeviceMonitorProxy,
    req: &'a mut SetCountryRequest,
) {
    let status = svc.set_country(req).await.expect("calling set_country");
    assert_eq!(status, ZX_OK);
    receiver.await.expect("error receiving set_country_helper mpsc message");
}

/// Issue service.fidl:SetCountry() protocol to Wlanstack's service with a test country code.
/// Test two things:
///  - If wlantap PHY device received the specified test country code
///  - If the SetCountry() returned successfully (ZX_OK).
#[fuchsia::test]
async fn set_country() {
    let mut helper = test_utils::TestHelper::begin_test(
        default_wlantap_config_client(),
        WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(false),
            with_policy: Some(false),
            ..Default::default()
        },
    )
    .await;

    let svc = connect_to_protocol_at::<DeviceMonitorMarker>(helper.test_ns_prefix())
        .expect("Failed to connect to wlandevicemonitor");
    let phy_list = svc.list_phys().await.unwrap();
    assert!(phy_list.len() > 0, "No PHYs present!");
    let phy_id = phy_list[0];

    // Set the country and await a signal from the event handler via `sender`.
    const ALPHA2: &[u8; 2] = b"RS";
    let mut req = SetCountryRequest { phy_id, alpha2: *ALPHA2 };
    let (sender, receiver) = oneshot::channel();
    let set_country_and_await_match = pin!(set_country_and_await_match(receiver, &svc, &mut req));

    helper
        .run_until_complete_or_timeout(
            zx::MonotonicDuration::INFINITE, // Unlimited timeout. Await `set_country` in the event handler.
            "wlanstack_dev_svc set_country",
            event::on_set_country(
                event::extract(|alpha2: [u8; 2]| assert_eq!(alpha2, *ALPHA2))
                    .and(event::once(|_, _| sender.send(())))
                    .expect("failed to send completion signal"),
            ),
            set_country_and_await_match,
        )
        .await;
}
