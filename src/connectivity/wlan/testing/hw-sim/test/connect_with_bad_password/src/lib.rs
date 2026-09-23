// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use ieee80211::{Bssid, Ssid};
use log::info;
use std::pin::pin;
use std::sync::LazyLock;
use wlan_common::bss::Protection;
use wlan_common::ie::rsn::cipher::{CIPHER_CCMP_128, CIPHER_TKIP, Cipher};
use wlan_hw_sim::*;

static BSSID: LazyLock<Bssid> = LazyLock::new(|| Bssid::from(*b"wpa2ok"));
const AUTHENTICATOR_PASSWORD: &str = "goodpassword";
const SUPPLICANT_PASSWORD: &str = "badpassword";

async fn connect_and_wait_for_failure(
    ssid: &Ssid,
    supplicant: Supplicant<'_>,
    expected_failure: fidl_policy::DisconnectStatus,
) {
    let credential = password_or_psk_to_policy_credential(supplicant.password);
    save_network(supplicant.controller, ssid, supplicant.security_type, credential.clone()).await;
    let network_identifier =
        fidl_policy::NetworkIdentifier { ssid: ssid.to_vec(), type_: supplicant.security_type };
    await_failed(supplicant.state_update_stream, network_identifier.clone(), expected_failure)
        .await;
    forget_network(supplicant.controller, ssid, supplicant.security_type).await;
}

async fn fail_to_connect_or_timeout(
    helper: &mut test_utils::TestHelper,
    timeout: zx::MonotonicDuration,
    ssid: &Ssid,
    bssid: &Bssid,
    protection: &Protection,
    cipher: Cipher,
    password: &str,
    supplicant: Supplicant<'_>,
    expected_failure: fidl_policy::DisconnectStatus,
) {
    let authenticator =
        create_authenticator(bssid, ssid, password, cipher, *protection, *protection);
    let connect = pin!(connect_and_wait_for_failure(ssid, supplicant, expected_failure));
    info!(
        "Attempting to connect to a network with {:?} protection **using an incorrect password**.",
        protection
    );
    connect_or_timeout_with(helper, timeout, ssid, bssid, protection, Some(authenticator), connect)
        .await;
    info!("OK: invalid connection parameters failed as expected.");
}

/// Test a client fails to connect to a network if the wrong credential type is
/// provided by the user. In particular, this occurs when a password should have
/// been provided and was not, or vice-versa.
#[fuchsia::test]
async fn connect_with_bad_password() {
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

    let (client_controller, mut client_state_update_stream) =
        init_client_controller(helper.test_ns_prefix()).await;
    let mut supplicant = Supplicant {
        controller: &client_controller,
        state_update_stream: &mut client_state_update_stream,
        security_type: fidl_policy::SecurityType::None,
        password: Some(SUPPLICANT_PASSWORD),
    };

    // Test a client fails to connect to a network protected by WPA3-Personal if the wrong password
    // is provided. The DisconnectStatus::CredentialsFailed status should be returned by policy.
    fail_to_connect_or_timeout(
        &mut helper,
        zx::MonotonicDuration::from_seconds(60),
        &Ssid::try_from("wpa3network").unwrap(),
        &BSSID,
        &Protection::Wpa3Personal,
        CIPHER_CCMP_128,
        AUTHENTICATOR_PASSWORD,
        Supplicant { security_type: fidl_policy::SecurityType::Wpa3, ..supplicant.reborrow() },
        fidl_policy::DisconnectStatus::ConnectionFailed,
    )
    .await;

    // Test a client fails to connect to a network protected by WPA2-PSK if the wrong password is
    // provided. The DisconnectStatus::CredentialsFailed status should be returned by policy.
    fail_to_connect_or_timeout(
        &mut helper,
        zx::MonotonicDuration::from_seconds(30),
        &Ssid::try_from("wpa2network").unwrap(),
        &BSSID,
        &Protection::Wpa2Personal,
        CIPHER_CCMP_128,
        AUTHENTICATOR_PASSWORD,
        Supplicant { security_type: fidl_policy::SecurityType::Wpa2, ..supplicant.reborrow() },
        fidl_policy::DisconnectStatus::CredentialsFailed,
    )
    .await;

    // Test a client fails to connect to a network protected by WPA1-PSK if the wrong password is
    // provided. The DisconnectStatus::CredentialsFailed status should be returned by policy.
    fail_to_connect_or_timeout(
        &mut helper,
        zx::MonotonicDuration::from_seconds(30),
        &Ssid::try_from("wpa1network").unwrap(),
        &BSSID,
        &Protection::Wpa1,
        CIPHER_TKIP,
        AUTHENTICATOR_PASSWORD,
        Supplicant { security_type: fidl_policy::SecurityType::Wpa, ..supplicant },
        fidl_policy::DisconnectStatus::CredentialsFailed,
    )
    .await;
}
