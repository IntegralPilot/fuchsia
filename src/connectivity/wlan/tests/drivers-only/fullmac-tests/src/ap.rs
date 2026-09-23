// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FullmacDriverFixture;
use assert_matches::assert_matches;
use drivers_only_common::sme_helpers::{
    self, DEFAULT_OPEN_AP_CONFIG, random_password, random_ssid,
};
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fullmac_helpers::config::FullmacDriverConfig;
use fullmac_helpers::recorded_request_stream::FullmacRequest;
use rand::seq::IndexedRandom as _;
use wlan_common::ie::rsn::rsne;

/// Many tests require a started BSS. This helper function creates and starts an AP in the test
/// realm and returns the ApSmeProxy, FullmacDriverFixture, and GenericSmeProxy.
async fn setup_test_bss_started(
    driver_config: FullmacDriverConfig,
    ap_config: &fidl_sme::ApConfig,
) -> (fidl_sme::ApSmeProxy, FullmacDriverFixture) {
    // This is wrapped in a Box::pin because otherwise the compiler complains about the future
    // being too large.
    Box::pin(async {
        let mut fullmac_driver =
            FullmacDriverFixture::create(driver_config).await;
        let ap_sme_proxy = sme_helpers::get_ap_sme(&fullmac_driver.generic_sme_proxy).await;

        let ap_fut = ap_sme_proxy.start(&ap_config);
        let driver_fut = async {
            assert_matches!(fullmac_driver.request_stream.next().await,
                fidl_fullmac::WlanFullmacImpl_Request::StartBss { payload: _, responder } => {
                    responder.send().expect("Could not respond to StartBss");
            });

            fullmac_driver
                .ifc_proxy
                .start_conf(&fidl_fullmac::WlanFullmacImplIfcStartConfRequest {
                    result_code: Some(fidl_fullmac::StartResult::Success),
                    ..Default::default()
                })
                .await
                .expect("Could not send StartConf");

            assert_matches!(fullmac_driver.request_stream.next().await,
                fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload:_ , responder } => {
                    responder.send().expect("Could not respond to OnLinkStateChanged");
            });
        };

        let (_, _) = futures::join!(ap_fut, driver_fut);

        fullmac_driver.request_stream.clear_history();
        (ap_sme_proxy, fullmac_driver)
    }).await
}

#[fuchsia::test]
async fn test_start_2ghz_bss_success() {
    let mut fullmac_driver = FullmacDriverFixture::create(FullmacDriverConfig::default_ap()).await;
    let ap_sme_proxy = sme_helpers::get_ap_sme(&fullmac_driver.generic_sme_proxy).await;

    let phy_types = [
        fidl_ieee80211::WlanPhyType::Dsss,
        fidl_ieee80211::WlanPhyType::Hr,
        fidl_ieee80211::WlanPhyType::Ofdm,
        fidl_ieee80211::WlanPhyType::Erp,
        fidl_ieee80211::WlanPhyType::Ht,
    ];

    // channel support is defined by fullmac driver config
    let driver_band_cap = &fullmac_driver.config.query_info.band_caps.as_ref().unwrap()[0];
    let supported_channels = &driver_band_cap.primary_channels.as_ref().unwrap();

    let chosen_channel = *supported_channels.choose(&mut rand::rng()).unwrap();
    let sme_ap_config = fidl_sme::ApConfig {
        ssid: random_ssid(),
        password: random_password(),
        radio_cfg: fidl_sme::RadioConfig {
            phy: *phy_types.choose(&mut rand::rng()).unwrap(),
            primary: chosen_channel,
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
        },
    };

    let ap_fut = ap_sme_proxy.start(&sme_ap_config);
    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::StartBss { payload: _, responder } => {
                responder.send().expect("Could not respond to StartBss");
        });

        fullmac_driver
            .ifc_proxy
            .start_conf(&fidl_fullmac::WlanFullmacImplIfcStartConfRequest {
                result_code: Some(fidl_fullmac::StartResult::Success),
                ..Default::default()
            })
            .await
            .expect("Could not send StartConf");

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload:_ , responder } => {
                responder.send().expect("Could not respond to OnLinkStateChanged");
        });
    };

    let (sme_start_result, _) = futures::join!(ap_fut, driver_fut);
    assert_eq!(
        sme_start_result.expect("Error on call to SME start"),
        fidl_sme::StartApResultCode::Success
    );

    let fullmac_request_history = fullmac_driver.request_stream.history();

    assert_eq!(
        fullmac_request_history[0],
        FullmacRequest::StartBss(fidl_fullmac::WlanFullmacImplStartBssRequest {
            ssid: Some(sme_ap_config.ssid.clone()),
            bss_type: Some(fidl_ieee80211::BssType::Infrastructure),
            beacon_period: Some(100),
            dtim_period: Some(2),
            primary: Some(sme_ap_config.radio_cfg.primary),
            rsne: Some(rsne::Rsne::wpa2_rsne_with_caps(rsne::RsnCapabilities(0)).into_bytes()),
            vendor_ie: Some(vec![]),
            ..Default::default()
        })
    );

    assert_eq!(
        fullmac_request_history[1],
        FullmacRequest::OnLinkStateChanged(
            fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest {
                online: Some(true),
                ..Default::default()
            }
        )
    );

    // Check AP status to see that SME reports that an AP is running.
    // Driver does not take part in this interaction.
    let running_ap =
        ap_sme_proxy.status().await.expect("Could not get ApSme status").running_ap.unwrap();
    assert_eq!(
        running_ap.as_ref(),
        &fidl_sme::Ap {
            ssid: sme_ap_config.ssid.clone(),
            channel: sme_ap_config.radio_cfg.primary.number,
            num_clients: 0,
        }
    );
}

#[fuchsia::test]
async fn test_start_bss_fail_non_ascii_password() {
    let fullmac_driver = FullmacDriverFixture::create(FullmacDriverConfig::default_ap()).await;
    let ap_sme_proxy = sme_helpers::get_ap_sme(&fullmac_driver.generic_sme_proxy).await;

    let sme_ap_config = fidl_sme::ApConfig {
        ssid: random_ssid(),
        password: vec![1, 2, 3],
        radio_cfg: fidl_sme::RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ofdm,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 1,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
        },
    };

    assert_eq!(
        ap_sme_proxy.start(&sme_ap_config).await.expect("Could not start AP"),
        fidl_sme::StartApResultCode::InvalidArguments
    );
}

#[fuchsia::test]
async fn test_start_bss_fail_bad_channel() {
    let fullmac_driver = FullmacDriverFixture::create(FullmacDriverConfig::default_ap()).await;
    let ap_sme_proxy = sme_helpers::get_ap_sme(&fullmac_driver.generic_sme_proxy).await;

    let sme_ap_config = fidl_sme::ApConfig {
        ssid: random_ssid(),
        password: vec![],
        radio_cfg: fidl_sme::RadioConfig {
            phy: fidl_ieee80211::WlanPhyType::Ofdm,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::FiveGhz,
                number: 27,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
        },
    };

    assert_eq!(
        ap_sme_proxy.start(&sme_ap_config).await.expect("Could not start AP"),
        fidl_sme::StartApResultCode::InvalidArguments
    );
}

#[fuchsia::test]
async fn test_stop_bss() {
    let (ap_sme_proxy, mut fullmac_driver) =
        setup_test_bss_started(FullmacDriverConfig::default_ap(), &DEFAULT_OPEN_AP_CONFIG).await;

    let ap_fut = ap_sme_proxy.stop();
    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
           fidl_fullmac::WlanFullmacImpl_Request::StopBss { payload: _, responder } => {
               responder.send().expect("Could not respond to StopBss");
        });

        fullmac_driver
            .ifc_proxy
            .stop_conf(&fidl_fullmac::WlanFullmacImplIfcStopConfRequest {
                result_code: Some(fidl_fullmac::StopResult::Success),
                ..Default::default()
            })
            .await
            .expect("Could not send StopConf");

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload:_ , responder } => {
                responder.send().expect("Could not respond to OnLinkStateChanged");
        });
    };

    let (ap_stop_result, _) = futures::join!(ap_fut, driver_fut);
    assert_eq!(
        ap_stop_result.expect("SME error when calling stop"),
        fidl_sme::StopApResultCode::Success
    );

    let fullmac_request_history = fullmac_driver.request_stream.history();
    assert_eq!(
        fullmac_request_history[0],
        FullmacRequest::StopBss(fidl_fullmac::WlanFullmacImplStopBssRequest {
            ssid: Some(DEFAULT_OPEN_AP_CONFIG.ssid.clone()),
            ..Default::default()
        })
    );

    assert_eq!(
        fullmac_request_history[1],
        FullmacRequest::OnLinkStateChanged(
            fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest {
                online: Some(false),
                ..Default::default()
            }
        )
    );
}

#[fuchsia::test]
async fn test_remote_client_connected_open() {
    let (ap_sme_proxy, mut fullmac_driver) =
        setup_test_bss_started(FullmacDriverConfig::default_ap(), &DEFAULT_OPEN_AP_CONFIG).await;

    let remote_sta_address: [u8; 6] = rand::random();
    fullmac_driver
        .ifc_proxy
        .auth_ind(&fidl_fullmac::WlanFullmacImplIfcAuthIndRequest {
            peer_sta_address: Some(remote_sta_address.clone()),
            auth_type: Some(fidl_fullmac::WlanAuthType::OpenSystem),
            ..Default::default()
        })
        .await
        .expect("Could not send AuthInd");

    assert_matches!(fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::AuthResp { payload: _, responder } => {
            responder.send().expect("Could not respond to AuthResp");
    });

    fullmac_driver
        .ifc_proxy
        .assoc_ind(&fidl_fullmac::WlanFullmacImplIfcAssocIndRequest {
            peer_sta_address: Some(remote_sta_address.clone()),
            listen_interval: Some(100),
            ssid: Some(DEFAULT_OPEN_AP_CONFIG.ssid.clone()),
            rsne: Some(vec![]),
            vendor_ie: Some(vec![]),
            ..Default::default()
        })
        .await
        .expect("Could not send AssocInd");

    assert_matches!(fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::AssocResp { payload: _, responder } => {
            responder.send().expect("Could not respond to AssocResp");
    });

    // Check AP status to see that SME reports that an AP is running.
    // Driver does not take part in this interaction.
    let running_ap =
        ap_sme_proxy.status().await.expect("Could not get ApSme status").running_ap.unwrap();
    assert_eq!(
        running_ap.as_ref(),
        &fidl_sme::Ap {
            ssid: DEFAULT_OPEN_AP_CONFIG.ssid.clone(),
            channel: DEFAULT_OPEN_AP_CONFIG.radio_cfg.primary.number,
            num_clients: 1,
        }
    );

    let fullmac_request_history = fullmac_driver.request_stream.history();
    assert_eq!(
        fullmac_request_history[0],
        FullmacRequest::AuthResp(fidl_fullmac::WlanFullmacImplAuthRespRequest {
            peer_sta_address: Some(remote_sta_address.clone()),
            result_code: Some(fidl_fullmac::WlanAuthResult::Success),
            ..Default::default()
        })
    );

    let assoc_resp =
        assert_matches!(&fullmac_request_history[1], FullmacRequest::AssocResp(resp) => resp);
    assert_eq!(assoc_resp.peer_sta_address, Some(remote_sta_address.clone()));
    assert_eq!(assoc_resp.result_code, Some(fidl_fullmac::WlanAssocResult::Success));
    // Note: association id is not checked since SME can pick any value.
}
