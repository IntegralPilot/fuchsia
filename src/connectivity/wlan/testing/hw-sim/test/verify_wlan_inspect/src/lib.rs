// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Error, format_err};
use diagnostics_assertions::{AnyProperty, assert_data_tree};
use diagnostics_hierarchy::DiagnosticsHierarchy;
use diagnostics_reader::{ArchiveReader, ComponentSelector};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_policy as fidl_policy;
use fidl_test_wlan_realm::WlanConfig;
use ieee80211::Bssid;
use std::pin::pin;
use std::sync::LazyLock;
use wlan_common::bss::Protection;
use wlan_common::channel::{Bandwidth, Channel};
use wlan_hw_sim::event::{Handler, action};
use wlan_hw_sim::*;

static BSSID: LazyLock<Bssid> = LazyLock::new(|| Bssid::from([0x62, 0x73, 0x73, 0x66, 0x6f, 0x6f]));

#[rustfmt::skip]
const WSC_IE_BODY: &'static [u8] = &[
    0x10, 0x4a, 0x00, 0x01, 0x10, // Version
    0x10, 0x44, 0x00, 0x01, 0x02, // WiFi Protected Setup State
    0x10, 0x57, 0x00, 0x01, 0x01, // AP Setup Locked
    0x10, 0x3b, 0x00, 0x01, 0x03, // Response Type
    // UUID-E
    0x10, 0x47, 0x00, 0x10,
    0x3b, 0x3b, 0xe3, 0x66, 0x80, 0x84, 0x4b, 0x03,
    0xbb, 0x66, 0x45, 0x2a, 0xf3, 0x00, 0x59, 0x22,
    // Manufacturer
    0x10, 0x21, 0x00, 0x15,
    0x41, 0x53, 0x55, 0x53, 0x54, 0x65, 0x6b, 0x20, 0x43, 0x6f, 0x6d, 0x70,
    0x75, 0x74, 0x65, 0x72, 0x20, 0x49, 0x6e, 0x63, 0x2e,
    // Model name
    0x10, 0x23, 0x00, 0x08, 0x52, 0x54, 0x2d, 0x41, 0x43, 0x35, 0x38, 0x55,
    // Model number
    0x10, 0x24, 0x00, 0x03, 0x31, 0x32, 0x33,
    // Serial number
    0x10, 0x42, 0x00, 0x05, 0x31, 0x32, 0x33, 0x34, 0x35,
    // Primary device type
    0x10, 0x54, 0x00, 0x08, 0x00, 0x06, 0x00, 0x50, 0xf2, 0x04, 0x00, 0x01,
    // Device name
    0x10, 0x11, 0x00, 0x0b,
    0x41, 0x53, 0x55, 0x53, 0x20, 0x52, 0x6f, 0x75, 0x74, 0x65, 0x72,
    // Config methods
    0x10, 0x08, 0x00, 0x02, 0x20, 0x0c,
    // Vendor extension
    0x10, 0x49, 0x00, 0x06, 0x00, 0x37, 0x2a, 0x00, 0x01, 0x20,
];

const REALM_NAME: &str = "verify_wlan_inspect";

/// Test a client can connect to a network with no protection by simulating an AP that sends out
/// hard coded authentication and association response frames.
#[fuchsia::test]
async fn verify_wlan_inspect() {
    let mut helper = test_utils::TestHelper::begin_test(
        default_wlantap_config_client(),
        WlanConfig {
            use_legacy_privacy: Some(false),
            with_regulatory_region: Some(true),
            with_policy: Some(true),
            name: Some(REALM_NAME.to_owned()),
            ..Default::default()
        },
    )
    .await;

    let policy_moniker = format!("test_realm_factory/realm_builder\\:{}/wlancfg", REALM_NAME);
    let devicemonitor_moniker =
        format!("test_realm_factory/realm_builder\\:{}/wlandevicemonitor", REALM_NAME);

    let () = loop_until_iface_is_found(&mut helper).await;

    let (client_controller, mut client_state_update_stream) =
        wlan_hw_sim::init_client_controller(helper.test_ns_prefix()).await;
    let security_type = fidl_policy::SecurityType::None;
    {
        let phy = helper.proxy();
        let channel = Channel::new(1, Bandwidth::Cbw20, fidl_ieee80211::WlanBand::TwoGhz);
        let protection = Protection::Open;
        let probes = [ProbeResponse {
            channel,
            bssid: *BSSID,
            ssid: AP_SSID.clone(),
            protection,
            rssi_dbm: -10,
            wsc_ie: Some(WSC_IE_BODY.to_vec()),
        }];

        let connect_future = pin!(async {
            save_network(
                &client_controller,
                &AP_SSID,
                security_type,
                password_or_psk_to_policy_credential::<String>(None),
            )
            .await;
            let id =
                fidl_policy::NetworkIdentifier { ssid: AP_SSID.to_vec(), type_: security_type };
            wait_until_client_state(&mut client_state_update_stream, |update| {
                has_id_and_state(update, &id, fidl_policy::ConnectionState::Connected)
            })
            .await;
        });
        let () = helper
            .run_until_complete_or_timeout(
                zx::MonotonicDuration::from_seconds(240),
                format!("connecting to {} ({})", AP_SSID.to_string_not_redactable(), *BSSID),
                event::on_scan(action::send_advertisements_and_scan_completion(&phy, probes))
                    .or(event::on_transmit(action::connect_with_open_authentication(
                        &phy,
                        &AP_SSID,
                        &BSSID,
                        &channel,
                        &protection,
                    )))
                    .expect("failed to scan and associate"),
                connect_future,
            )
            .await;
    }

    let policy_hierarchy =
        get_inspect_hierarchy(&policy_moniker).await.expect("expect Inspect data");
    assert_data_tree!(policy_hierarchy, root: contains {
        external: {
            client_stats: contains {
                disconnect_events: {},
                connection_status: contains {
                    connected_network: {
                        rssi_dbm: AnyProperty,
                        snr_db: AnyProperty,
                        wsc: {
                            manufacturer: "ASUSTek Computer Inc.",
                            model_name: "RT-AC58U",
                            model_number: "123",
                        }
                    }
                }
            },
        },
    });

    let monitor_hierarchy =
        get_inspect_hierarchy(&devicemonitor_moniker).await.expect("expect Inspect data");
    assert_data_tree!(monitor_hierarchy, root: contains {
        device_events: contains {
            "0": contains {},
        },
        ifaces: contains {
            "0": contains {
                usme: contains {
                    last_pulse: contains {
                        status: contains {
                            status_str: "connected",
                            connected_to: contains {
                                bssid: BSSID.to_string(),
                                ssid: AP_SSID.to_string(),
                                wsc: {
                                    manufacturer: "ASUSTek Computer Inc.",
                                    model_name: "RT-AC58U",
                                    model_number: "123",
                                }
                            }
                        }
                    },
                    state_events: contains {
                        "0": contains {},
                        "1": contains {},
                    },
                },
            },
        },
    });

    forget_network(&client_controller, &AP_SSID, fidl_policy::SecurityType::None).await;

    let id = fidl_policy::NetworkIdentifier { ssid: AP_SSID.to_vec(), type_: security_type };
    wait_until_client_state(&mut client_state_update_stream, |update| {
        has_id_and_state(update, &id, fidl_policy::ConnectionState::Disconnected)
    })
    .await;

    let policy_hierarchy =
        get_inspect_hierarchy(&policy_moniker).await.expect("expect Inspect data");
    assert_data_tree!(policy_hierarchy, root: contains {
        external: {
            client_stats: contains {
                disconnect_events: {
                    "0": {
                        "@time": AnyProperty,
                        network: {
                            channel: {
                                primary: 1u64,
                            },
                        },
                        flattened_reason_code: AnyProperty,
                        locally_initiated: true,
                    }
                },
                connection_status: contains {},
            }
        },
    });

    let monitor_hierarchy =
        get_inspect_hierarchy(&devicemonitor_moniker).await.expect("expect Inspect data");
    assert_data_tree!(monitor_hierarchy, root: contains {
        device_events: contains {
            "0": contains {},
        },
        ifaces: contains {
            "0": contains {
                usme: contains {
                    last_pulse: contains {
                        status: contains {
                            status_str: "idle",
                            prev_connected_to: contains {
                                bssid: BSSID.to_string(),
                                ssid: AP_SSID.to_string(),
                                wsc: {
                                    manufacturer: "ASUSTek Computer Inc.",
                                    model_name: "RT-AC58U",
                                    model_number: "123",
                                }
                            }
                        }
                    },
                    state_events: contains {
                        "0": contains {},
                        "1": contains {},
                    }
                },
            },
        },
    });
}

async fn get_inspect_hierarchy(component: &str) -> Result<DiagnosticsHierarchy, Error> {
    ArchiveReader::inspect()
        .add_selector(ComponentSelector::new(vec![component.to_string()]))
        .snapshot()
        .await?
        .into_iter()
        .next()
        .and_then(|result| result.payload)
        .ok_or_else(|| format_err!("expected one inspect hierarchy"))
}
