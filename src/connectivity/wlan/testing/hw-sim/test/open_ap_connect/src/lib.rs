// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_test_wlan_realm::WlanConfig;
use futures::channel::oneshot;
use std::panic;
use wlan_hw_sim::event::buffered::{ActionFrame, AssocRespFrame, AuthFrame, Buffered, MgmtFrame};
use wlan_hw_sim::event::{Handler, branch};
use wlan_hw_sim::*;

/// Test WLAN AP implementation by simulating a client that sends out authentication and
/// association *request* frames. Verify AP responds correctly with authentication and
/// association *response* frames, respectively.
#[fuchsia::test]
async fn open_ap_connect() {
    // --- start test data block

    // frame 1 and 3 from ios12.1-connect-open-ap.pcapng
    #[rustfmt::skip]
    const AUTH_REQ_HEX: &str = "b0003a0170f11c052d7fdca90435e58c70f11c052d7ff07d000001000000dd0b0017f20a00010400000000dd09001018020000100000ed7895e7";
    let auth_req = hex::decode(AUTH_REQ_HEX).expect("fail to parse auth req hex");

    #[rustfmt::skip]
    const ASSOC_REQ_HEX: &str = "00003a0170f11c052d7fdca90435e58c70f11c052d7f007e210414000014465543485349412d544553542d4b4945542d4150010882848b962430486c32040c121860210202142402010ddd0b0017f20a00010400000000dd09001018020000100000debda9bb";
    let assoc_req = hex::decode(ASSOC_REQ_HEX).expect("fail to parse assoc req hex");
    // -- end test data block

    // Start up the AP
    let network_config = NetworkConfigBuilder::open().ssid(&AP_SSID);
    let mut helper = test_utils::TestHelper::begin_ap_test(
        default_wlantap_config_ap(),
        network_config,
        WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(true),
            with_policy: Some(true),
            ..Default::default()
        },
    )
    .await;

    // (client->ap) send a mock auth req
    let proxy = helper.proxy();
    proxy.rx(&auth_req, &rx_info_with_default_ap()).expect("cannot send auth req frame");

    // (ap->client) verify auth response frame was sent
    verify_auth_resp(&mut helper).await;

    // (client->ap) send a mock assoc req
    let proxy = helper.proxy();
    proxy.rx(&assoc_req, &rx_info_with_default_ap()).expect("cannot send assoc req frame");

    // (ap->client) verify assoc response frame was sent
    verify_assoc_resp(&mut helper).await;
}

async fn verify_auth_resp(helper: &mut test_utils::TestHelper) {
    let (sender, receiver) = oneshot::channel::<()>();
    helper
        .run_until_complete_or_timeout(
            zx::MonotonicDuration::from_seconds(5),
            "waiting for authentication response",
            event::on_transmit(event::extract(|frame: Buffered<AuthFrame>| {
                let frame = frame.get();
                assert_eq!(
                    { frame.auth_hdr.status_code },
                    fidl_ieee80211::StatusCode::Success.into()
                );
            }))
            .and(event::once(|_, _| sender.send(()).expect("failed to send completion message"))),
            receiver,
        )
        .await
        .expect("failed to verify authentication response");
}

async fn verify_assoc_resp(helper: &mut test_utils::TestHelper) {
    let (sender, receiver) = oneshot::channel::<()>();
    helper
        .run_until_complete_or_timeout(
            zx::MonotonicDuration::from_seconds(5),
            "waiting for association response",
            event::on_transmit(branch::or((
                event::extract(|_: Buffered<ActionFrame<false>>| {}),
                event::until_first_match(branch::or((
                    event::extract(|frame: Buffered<AssocRespFrame>| {
                        let frame = frame.get();
                        assert_eq!(
                            { frame.assoc_resp_hdr.status_code },
                            fidl_ieee80211::StatusCode::Success.into(),
                        );
                    })
                    .and(event::once(|_, _| {
                        sender.send(()).expect("failed to send completion message")
                    })),
                    event::extract(|_: Buffered<MgmtFrame>| {
                        panic!("unexpected management frame (out of order)");
                    }),
                ))),
            ))),
            receiver,
        )
        .await
        .expect("failed to verify association response");
}
