// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_fuchsia_wlan_tap as fidl_tap;
use fidl_test_wlan_realm::WlanConfig;
use fuchsia_async::Task;
use futures::channel::oneshot;
use ieee80211::{Bssid, Ssid};
use std::pin::pin;
use wlan_common::bss::Protection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_common::ie::rsn::cipher::CIPHER_CCMP_128;
use wlan_hw_sim::event::action::{self, AuthenticationControl, AuthenticationTap};
use wlan_hw_sim::event::{Handler, branch};
use wlan_hw_sim::*;
use wlan_rsn::rsna::UpdateSink;

async fn run_policy_and_assert_transparent_reconnect(
    test_ns_prefix: &str,
    ssid: &'static Ssid,
    // TODO(https://fxbug.dev/42080534): Unify security protocol types and respect this parameter.
    _protection: &Protection,
    password: Option<&str>,
    receiver: oneshot::Receiver<()>,
) {
    // TODO(https://fxbug.dev/42080534): Respect the parameter of the same name.
    let protection = fidl_policy::SecurityType::Wpa2;
    // Connect to the client policy service and get a client controller.
    let (_client_controller, mut client_state_updates) = save_network_and_wait_until_connected(
        test_ns_prefix,
        ssid,
        protection,
        password_or_psk_to_policy_credential(password),
    )
    .await;

    let task = Task::spawn(async move {
        let id = fidl_policy::NetworkIdentifier { ssid: ssid.to_vec(), type_: protection };
        // Check updates from the policy. If we see disconnect, panic.
        wait_until_client_state(&mut client_state_updates, |update| {
            has_id_and_state(update, &id, fidl_policy::ConnectionState::Disconnected)
        })
        .await;
        panic!("policy observed a disconnect during reassocation");
    });
    receiver.await.expect("waiting for reassociation signal");
    drop(task);
}

fn scan_and_reassociate<'h>(
    phy: &'h fidl_tap::WlantapPhyProxy,
    ssid: &'h Ssid,
    bssid: &'h Bssid,
    channel: &'h Channel,
    protection: &'h Protection,
    control: &'h mut AuthenticationControl,
    sender: oneshot::Sender<()>,
) -> impl Handler<(), fidl_tap::WlantapPhyEvent> + 'h {
    let beacons = [Beacon {
        ssid: ssid.clone(),
        bssid: *bssid,
        channel: *channel,
        protection: *protection,
        rssi_dbm: -30,
    }];
    let mut has_previously_associated = false;
    let mut sender = Some(sender);
    let tap = AuthenticationTap {
        control,
        handler: action::authenticate_with_control_state().try_and(event::matched(
            move |control: &mut AuthenticationControl, _| {
                use wlan_rsn::rsna::SecAssocStatus::EssSaEstablished;
                use wlan_rsn::rsna::SecAssocUpdate::Status;

                // TODO(https://fxbug.dev/42148729): Use `Vec::drain_filter` instead when the API is
                //                        stabilized.
                let mut index = 0;
                while index < control.updates.len() {
                    if let Status(EssSaEstablished) = &control.updates[index] {
                        // Disassociate after the first `EssSaEstablished` update.
                        if !has_previously_associated {
                            send_disassociate(
                                channel,
                                bssid,
                                fidl_ieee80211::ReasonCode::NoMoreStas,
                                phy,
                            )?;
                            control.authenticator.reset();
                            has_previously_associated = true;
                        } else {
                            sender
                                .take()
                                .map_or(Ok(()), |sender| sender.send(()))
                                .map_err(|_| format_err!("failed to send reassociation signal"))?;
                        }
                        control.updates.remove(index);
                    } else {
                        index += 1;
                    }
                }
                Ok(())
            },
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

/// Test a client can reconnect to a network protected by WPA2-PSK
/// after being disassociated without the policy layer noticing. SME
/// is capable of rebuilding the link on its own when the AP
/// disassociates the client. In this test, no data is being sent
/// after the link becomes up.
#[fuchsia::test]
async fn reconnect_to_wpa2_network() {
    let bssid: Bssid = Bssid::from(*b"wpa2ok");
    const PROTECTION: Protection = Protection::Wpa2Personal;
    const PASSWORD: &str = "wpa2good";

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
            PASSWORD,
            CIPHER_CCMP_128,
            PROTECTION,
            PROTECTION,
        ),
    };

    let test_ns_prefix = helper.test_ns_prefix().to_string();
    let run_policy_future = pin!(run_policy_and_assert_transparent_reconnect(
        &test_ns_prefix,
        &AP_SSID,
        &PROTECTION,
        Some(PASSWORD),
        receiver,
    ));
    helper
        .run_until_complete_or_timeout(
            zx::MonotonicDuration::from_seconds(30),
            format!("connecting to {} ({})", AP_SSID.to_string_not_redactable(), bssid),
            scan_and_reassociate(
                &phy,
                &AP_SSID,
                &bssid,
                &Channel::new(1, Bandwidth::Cbw20, fidl_fuchsia_wlan_ieee80211::WlanBand::TwoGhz),
                &PROTECTION,
                &mut control,
                sender,
            ),
            run_policy_future,
        )
        .await;
}
