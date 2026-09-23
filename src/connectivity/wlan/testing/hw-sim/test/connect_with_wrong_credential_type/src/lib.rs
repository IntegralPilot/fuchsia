// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use ieee80211::Bssid;
use log::info;
use std::pin::pin;
use std::sync::LazyLock;
use wlan_common::bss::Protection;
use wlan_common::ie::rsn::cipher::{CIPHER_CCMP_128, CIPHER_TKIP};
use wlan_hw_sim::*;

static BSSID: LazyLock<Bssid> = LazyLock::new(|| Bssid::from(*b"bessid"));
const AUTHENTICATOR_PASSWORD: &str = "goodpassword";

async fn connect_and_wait_for_failure(supplicant: Supplicant<'_>) {
    let credential = password_or_psk_to_policy_credential(supplicant.password);
    save_network(supplicant.controller, &AP_SSID, supplicant.security_type, credential.clone())
        .await;
    await_failed(
        supplicant.state_update_stream,
        fidl_policy::NetworkIdentifier { ssid: AP_SSID.to_vec(), type_: supplicant.security_type },
        fidl_policy::DisconnectStatus::ConnectionFailed,
    )
    .await;
    forget_network(supplicant.controller, &AP_SSID, supplicant.security_type).await;
    info!(
        "Connection failed as expected. TODO(https://fxbug.dev/42081001#c4): remove this logging"
    );
}

/// Test a client fails to connect to a network if the wrong credential type is
/// provided by the user. In particular, this occurs when a password should have
/// been provided and was not, or vice-versa.
#[fuchsia::test]
async fn connecting_to_aps_with_wrong_credential_types() {
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

    let (client_controller, mut client_state_update_stream) =
        init_client_controller(helper.test_ns_prefix()).await;
    let mut supplicant = Supplicant {
        controller: &client_controller,
        state_update_stream: &mut client_state_update_stream,
        security_type: fidl_policy::SecurityType::None,
        password: None,
    };
    let timeout: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(30);

    // Test a client fails to connect to a network protected by WPA2-PSK if no
    // password is provided. The DisconnectStatus::CredentialsFailed status should be
    // returned by policy.
    {
        let authenticator = create_authenticator(
            &BSSID,
            &AP_SSID,
            AUTHENTICATOR_PASSWORD,
            CIPHER_CCMP_128,
            Protection::Wpa2Personal,
            Protection::Wpa2Personal,
        );
        let connect = pin!(connect_and_wait_for_failure(supplicant.reborrow()));
        info!("Attempting to connect to a WPA2 network **without a password**.");
        connect_or_timeout_with(
            &mut helper,
            timeout,
            &AP_SSID,
            &BSSID,
            &Protection::Wpa2Personal,
            Some(authenticator),
            connect,
        )
        .await;
        info!("OK: invalid connection parameters failed as expected.");
    }

    // Test a client fails to connect to a network protected by WPA1-PSK if no
    // password is provided. The DisconnectStatus::CredentialsFailed status should be
    // returned by policy.
    {
        let authenticator = create_authenticator(
            &BSSID,
            &AP_SSID,
            AUTHENTICATOR_PASSWORD,
            CIPHER_TKIP,
            Protection::Wpa1,
            Protection::Wpa1,
        );
        let connect = pin!(connect_and_wait_for_failure(supplicant.reborrow()));
        info!("Attempting to connect to a WPA1 network **without a password**.");
        connect_or_timeout_with(
            &mut helper,
            timeout,
            &AP_SSID,
            &BSSID,
            &Protection::Wpa1,
            Some(authenticator),
            connect,
        )
        .await;
        info!("OK: invalid connection parameters failed as expected.");
    }

    // Test a client fails to connect to a network protected by WEP if no
    // password is provided. The DisconnectStatus::CredentialsFailed status should be
    // returned by policy.
    {
        let connect = pin!(connect_and_wait_for_failure(supplicant.reborrow()));
        info!("Attempting to connect to a WEP network **without a password**.");
        connect_or_timeout_with(
            &mut helper,
            timeout,
            &AP_SSID,
            &BSSID,
            &Protection::Wep,
            None,
            connect,
        )
        .await;
        info!("OK: invalid connection parameters failed as expected.");
    }

    // Test a client fails to connect to an open network if a
    // password is provided. The DisconnectStatus::CredentialsFailed status should be
    // returned by policy.
    {
        let connect = pin!(connect_and_wait_for_failure(Supplicant {
            security_type: fidl_policy::SecurityType::Wpa2,
            password: Some("password"),
            ..supplicant.reborrow()
        }));
        info!("Attempting to connect to an open network **with a password**.");
        connect_or_timeout_with(
            &mut helper,
            timeout,
            &AP_SSID,
            &BSSID,
            &Protection::Open,
            None,
            connect,
        )
        .await;
        info!("OK: invalid connection parameters failed as expected.");
    }
}
