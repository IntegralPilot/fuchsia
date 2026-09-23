// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_tap as fidl_tap;
use fidl_test_wlan_realm::WlanConfig;
use futures::channel::oneshot;
use futures::join;
use ieee80211::{Bssid, Ssid};
use std::pin::pin;
use wlan_common::bss::Protection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::ie::rsn::cipher::CIPHER_CCMP_128;
use wlan_hw_sim::event::action::{self, AuthenticationControl, AuthenticationTap};
use wlan_hw_sim::event::{Handler, branch};
use wlan_hw_sim::*;
use wlan_rsn::rsna::UpdateSink;

fn scan_and_connect<'h>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    ssid: &'h Ssid,
    bssid: &'h Bssid,
    channel: &'h Channel,
    protection: &'h Protection,
    control: &'h mut AuthenticationControl,
    trace: &'h mut UpdateSink,
    sender: oneshot::Sender<()>,
) -> impl Handler<(), fidl_tap::WlantapPhyEvent> + 'h {
    let beacons = [Beacon {
        ssid: ssid.clone(),
        bssid: *bssid,
        channel: *channel,
        protection: *protection,
        rssi_dbm: -30,
    }];
    let mut sender = Some(sender);
    let tap = AuthenticationTap {
        control,
        handler: branch::try_and((
            event::matched(|control: &mut AuthenticationControl, _| {
                // Copy updates into the trace buffer.
                Ok(trace.extend(control.updates.iter().cloned()))
            }),
            action::authenticate_with_control_state(),
            event::matched(move |control: &mut AuthenticationControl, _| {
                use wlan_rsn::rsna::SecAssocStatus::EssSaEstablished;
                use wlan_rsn::rsna::SecAssocUpdate::Status;

                // Clear the update sink to prevent copying spurious updates into the trace.
                // Retaining updates is not required for the WPA2 EAPOL exchange, because
                // `TxEapolFrame` updates only occur after association.
                let mut result = Ok(());
                if control.updates.iter().any(|update| matches!(update, Status(EssSaEstablished))) {
                    // Defer errors so that updates are always cleared.
                    result = sender
                        .take()
                        .map_or(Ok(()), |sender| sender.send(()))
                        .map_err(|_| format_err!("failed to send association signal"));
                }
                control.updates.clear();
                result
            }),
        )),
    };
    branch::or((
        event::on_scan(action::send_advertisements_and_scan_completion(phy, beacons)),
        event::on_transmit(action::connect_with_authentication_tap(
            phy, ssid, bssid, channel, protection, tap,
        )),
    ))
    .expect("failed to scan and connect")
}

/// Test a client can connect to a network protected by WPA2-PSK by simulating an AP that
/// authenticates, associates, as well as initiating and completing EAPOL exchange.
/// In this test, no data is being sent after the link becomes up.
#[fuchsia::test]
async fn handle_tx_event_hooks() {
    let bssid: Bssid = Bssid::from(*b"wpa2ok");
    const PASSWORD: &str = "wpa2good";
    const PROTECTION: Protection = Protection::Wpa2Personal;

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
    let (sender, receiver) = oneshot::channel();
    let mut control = AuthenticationControl {
        updates: UpdateSink::new(),
        authenticator: create_authenticator(
            &bssid,
            &AP_SSID,
            &PASSWORD,
            CIPHER_CCMP_128,
            Protection::Wpa2Personal,
            Protection::Wpa2Personal,
        ),
    };
    let mut trace = UpdateSink::new();

    let test_ns_prefix = helper.test_ns_prefix().to_string();

    // Run Policy and wait for the client to connect and EssSa to establish.
    let run_policy_future = pin!(async {
        join!(
            save_network_and_wait_until_connected(
                &test_ns_prefix,
                &AP_SSID,
                fidl_policy::SecurityType::Wpa2,
                password_or_psk_to_policy_credential(Some(PASSWORD))
            ),
            async { receiver.await.expect("failed to receive association signal") }
        )
    });
    helper
        .run_until_complete_or_timeout(
            zx::MonotonicDuration::from_seconds(30),
            format!("connecting to {} ({:02X?})", AP_SSID.to_string_not_redactable(), bssid),
            scan_and_connect(
                &phy,
                &AP_SSID,
                &bssid,
                &Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz),
                &PROTECTION,
                &mut control,
                &mut trace,
                sender,
            ),
            run_policy_future,
        )
        .await;

    // The `scan_and_connect` event handler accumulates updates from the authentication update
    // sink into `trace`. If association succeeds, then the last six updates must be in the order
    // asserted here.
    use wlan_rsn::key::exchange::Key::{Gtk, Ptk};
    use wlan_rsn::rsna::SecAssocStatus::{EssSaEstablished, PmkSaEstablished};
    use wlan_rsn::rsna::SecAssocUpdate::{Key, Status, TxEapolKeyFrame};
    let n = trace.len();
    assert!(
        n >= 6,
        "expected six or more authentication updates but observed fewer:\n{:#?}",
        trace
    );
    assert!(matches!(trace[n - 6], Status(PmkSaEstablished)));
    assert!(matches!(trace[n - 5], TxEapolKeyFrame { .. }));
    assert!(matches!(trace[n - 4], TxEapolKeyFrame { .. }));
    assert!(matches!(trace[n - 3], Key(Ptk(..))));
    assert!(matches!(trace[n - 2], Key(Gtk(..))));
    assert!(matches!(trace[n - 1], Status(EssSaEstablished)));
}
