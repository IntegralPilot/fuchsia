// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::FullmacDriverFixture;
use assert_matches::assert_matches;
use drivers_only_common::sme_helpers;
use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_driver as fidl_driver_common;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_wlan_security;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_sme as fidl_sme;
use fullmac_helpers::config::FullmacDriverConfig;
use fullmac_helpers::recorded_request_stream::FullmacRequest;
use fullmac_helpers::{COMPATIBLE_OPEN_BSS, COMPATIBLE_WPA2_BSS, COMPATIBLE_WPA3_BSS};
use futures::StreamExt;
use ieee80211::{MacAddr, MacAddrBytes};
use wlan_common::random_fidl_bss_description;
use wlan_rsn::key::Tk;
use wlan_rsn::key::exchange::Key;
use wlan_rsn::rsna::{AuthStatus, SecAssocStatus, SecAssocUpdate, UpdateSink};

/// Many tests will want to start from a connected state, so this will create and start the test
/// realm, fullmac driver, and client SME, and then get the client SME into a connected state.
/// This will use COMPATIBLE_OPEN_BSS as the BssDescription for the connect call.
async fn setup_connected_to_open_bss(
    config: FullmacDriverConfig,
) -> (fidl_sme::ClientSmeProxy, fidl_sme::ConnectTransactionEventStream, FullmacDriverFixture) {
    // This is wrapped in a Box::pin because otherwise the compiler complains about the future
    // being too large.
    Box::pin(async {
        let mut fullmac_driver = FullmacDriverFixture::create(config).await;
        let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

        let client_fut = async {
            let (connect_txn, connect_txn_server) =
                fidl::endpoints::create_proxy::<fidl_sme::ConnectTransactionMarker>();

            let connect_req = fidl_sme::ConnectRequest {
                ssid: COMPATIBLE_OPEN_BSS.ssid.clone().into(),
                bss_description: COMPATIBLE_OPEN_BSS.clone().into(),
                multiple_bss_candidates: false,
                authentication: fidl_wlan_security::Authentication {
                    protocol: fidl_wlan_security::Protocol::Open,
                    credentials: None,
                },
                deprecated_scan_type: fidl_common::ScanType::Passive,
            };

            client_sme_proxy
                .connect(&connect_req, Some(connect_txn_server))
                .expect("Connect FIDL error.");

            connect_txn.take_event_stream()
        };

        let driver_fut = async {
            assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::Connect { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to Connect");
            });

            fullmac_driver
                .ifc_proxy
                .connect_conf(&fidl_fullmac::WlanFullmacImplIfcConnectConfRequest {
                    peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
                    result_code: Some(fidl_ieee80211::StatusCode::Success),
                    association_id: Some(0),
                    association_ies: Some(vec![]),
                    ..Default::default()
                })
                .await
                .expect("Failed to send ConnectConf");

            assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to OnLinkStateChanged");
            });
        };

        let (mut connect_txn_event_stream, _) = futures::join!(client_fut, driver_fut);
        assert_matches!(connect_txn_event_stream.next().await,
        Some(Ok(fidl_sme::ConnectTransactionEvent::OnConnectResult { result })) =>  {
            assert_eq!(result.code, fidl_ieee80211::StatusCode::Success);
        });

        // Don't include setup requests in request stream history().
        fullmac_driver.request_stream.clear_history();
        (client_sme_proxy, connect_txn_event_stream, fullmac_driver)
    })
    .await
}

#[fuchsia::test]
async fn test_scan_request_success() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

    let client_fut = async {
        client_sme_proxy
            .scan(&fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![],
            }))
            .await
            .expect("FIDL error")
            .expect("ScanRequest error")
    };

    let driver_fut = async {
        let txn_id = assert_matches!(fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::StartScan { payload, responder } => {
            responder
                .send()
                .expect("Failed to respond to StartScan");
            payload.txn_id.expect("No txn_id found")
        });

        let scan_result_list = vec![
            fidl_fullmac::WlanFullmacImplIfcOnScanResultRequest {
                txn_id: Some(txn_id),
                timestamp_nanos: Some(zx::MonotonicInstant::get().into_nanos()),
                bss: Some(random_fidl_bss_description!()),
                ..Default::default()
            },
            fidl_fullmac::WlanFullmacImplIfcOnScanResultRequest {
                txn_id: Some(txn_id),
                timestamp_nanos: Some(zx::MonotonicInstant::get().into_nanos() + 1),
                bss: Some(random_fidl_bss_description!()),
                ..Default::default()
            },
        ];

        for scan_result in &scan_result_list {
            fullmac_driver
                .ifc_proxy
                .on_scan_result(scan_result)
                .await
                .expect("Failed to send on_scan_result");
        }

        fullmac_driver
            .ifc_proxy
            .on_scan_end(&fidl_fullmac::WlanFullmacImplIfcOnScanEndRequest {
                txn_id: Some(txn_id),
                code: Some(fidl_fullmac::WlanScanResult::Success),
                ..Default::default()
            })
            .await
            .expect("Failed to send on_scan_end");

        scan_result_list
    };

    let (scan_result_vmo, expected_scan_result_list) = futures::join!(client_fut, driver_fut);
    let scan_result_list =
        wlan_common::scan::read_vmo(scan_result_vmo).expect("Could not read scan result VMO");

    assert_eq!(scan_result_list.len(), expected_scan_result_list.len());
    let expected_bss_descriptions: Vec<_> =
        expected_scan_result_list.iter().map(|scan_result| scan_result.bss.clone()).collect();

    for actual in scan_result_list {
        // TODO(https://g-issues.fuchsia.dev/issues/42164608):  SME ignores timestamps so they
        // aren't checked here.
        // NOTE: order of returned scans is not guaranteed.
        assert!(expected_bss_descriptions.contains(&Some(actual.bss_description)));
    }

    let scan_req = assert_matches!(&fullmac_driver.request_stream.history()[0], FullmacRequest::StartScan(req) => req);
    assert_eq!(scan_req.scan_type.unwrap(), fidl_fullmac::WlanScanType::Passive);
}

#[fuchsia::test]
async fn test_scan_request_error() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

    let client_fut = async {
        client_sme_proxy
            .scan(&fidl_sme::ScanRequest::Passive(fidl_sme::PassiveScanRequest {
                channels: vec![],
            }))
            .await
            .expect("FIDL error")
    };

    let driver_fut = async {
        let txn_id = assert_matches!(fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::StartScan { payload, responder } => {
            responder
                .send()
                .expect("Failed to respond to StartScan");
            payload.txn_id.expect("No txn_id found")
        });

        fullmac_driver
            .ifc_proxy
            .on_scan_end(&fidl_fullmac::WlanFullmacImplIfcOnScanEndRequest {
                txn_id: Some(txn_id),
                code: Some(fidl_fullmac::WlanScanResult::NotSupported),
                ..Default::default()
            })
            .await
            .expect("Failed to send on_scan_end");
    };

    let (scan_result, _) = futures::join!(client_fut, driver_fut);
    assert_eq!(scan_result.unwrap_err(), fidl_sme::ScanErrorCode::NotSupported);
}

#[fuchsia::test]
async fn test_open_connect_request_success() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

    let client_fut = async {
        let (connect_txn, connect_txn_server) =
            fidl::endpoints::create_proxy::<fidl_sme::ConnectTransactionMarker>();
        let mut connect_txn_event_stream = connect_txn.take_event_stream();

        let connect_req = fidl_sme::ConnectRequest {
            ssid: COMPATIBLE_OPEN_BSS.ssid.clone().into(),
            bss_description: COMPATIBLE_OPEN_BSS.clone().into(),
            multiple_bss_candidates: false,
            authentication: fidl_wlan_security::Authentication {
                protocol: fidl_wlan_security::Protocol::Open,
                credentials: None,
            },

            // Note: this field has no effect for fullmac drivers.
            deprecated_scan_type: fidl_common::ScanType::Passive,
        };

        client_sme_proxy
            .connect(&connect_req, Some(connect_txn_server))
            .expect("Connect FIDL error.");

        let connect_txn_event = connect_txn_event_stream
            .next()
            .await
            .expect("Connect event stream FIDL error")
            .expect("Connect txn returned error");

        // Returns the Connect result code.
        assert_matches!(connect_txn_event,
            fidl_sme::ConnectTransactionEvent::OnConnectResult { result } => {
                result
            }
        )
    };

    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::Connect { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to Connect");
        });

        fullmac_driver
            .ifc_proxy
            .connect_conf(&fidl_fullmac::WlanFullmacImplIfcConnectConfRequest {
                peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
                result_code: Some(fidl_ieee80211::StatusCode::Success),
                association_id: Some(0),
                association_ies: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("Failed to send ConnectConf");

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to OnLinkStateChanged");
        });
    };

    let (connect_result, _) = futures::join!(client_fut, driver_fut);

    assert_eq!(connect_result.code, fidl_ieee80211::StatusCode::Success);
    let fullmac_request_history = fullmac_driver.request_stream.history();

    // TODO(https://fxbug.dev/337074689): This is checked field by field because WEP key is initialized to some default
    // value determined by Banjo -> FIDL conversion code in wlanif. Instead of checking against
    // that default value, we ignore it in the test.
    let driver_connect_req =
        assert_matches!(&fullmac_request_history[0], FullmacRequest::Connect(req) => req.clone());
    assert_eq!(driver_connect_req.selected_bss.unwrap(), COMPATIBLE_OPEN_BSS.clone().into());
    assert_eq!(driver_connect_req.connect_failure_timeout.unwrap(), 60);
    assert_eq!(driver_connect_req.auth_type.unwrap(), fidl_fullmac::WlanAuthType::OpenSystem);

    // TODO(https://fxbug.dev/337074689): Check that these are None instead of empty vectors.
    assert_eq!(driver_connect_req.sae_password.unwrap(), vec![] as Vec<u8>);
    assert_eq!(driver_connect_req.security_ie.unwrap(), vec![] as Vec<u8>);

    assert_eq!(
        fullmac_request_history[1],
        FullmacRequest::OnLinkStateChanged(
            fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest {
                online: Some(true),
                ..Default::default()
            }
        )
    );
}

#[fuchsia::test]
async fn test_open_connect_request_error() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

    let client_fut = async {
        let (connect_txn, connect_txn_server) =
            fidl::endpoints::create_proxy::<fidl_sme::ConnectTransactionMarker>();
        let mut connect_txn_event_stream = connect_txn.take_event_stream();

        let connect_req = fidl_sme::ConnectRequest {
            ssid: COMPATIBLE_OPEN_BSS.ssid.clone().into(),
            bss_description: COMPATIBLE_OPEN_BSS.clone().into(),
            multiple_bss_candidates: false,
            authentication: fidl_wlan_security::Authentication {
                protocol: fidl_wlan_security::Protocol::Open,
                credentials: None,
            },

            // Note: this field has no effect for fullmac drivers.
            deprecated_scan_type: fidl_common::ScanType::Passive,
        };

        client_sme_proxy
            .connect(&connect_req, Some(connect_txn_server))
            .expect("Connect FIDL error.");

        let connect_txn_event = connect_txn_event_stream
            .next()
            .await
            .expect("Connect event stream FIDL error")
            .expect("Connect txn returned error");

        // Returns the Connect result code.
        assert_matches!(connect_txn_event,
            fidl_sme::ConnectTransactionEvent::OnConnectResult { result } => {
                result
            }
        )
    };

    let driver_fut = async {
        // The driver responds to the initial Connect request after it sends a failed ConnectConf.
        let connect_req_responder = assert_matches!(fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::Connect { payload: _, responder } => {
            responder
        });

        fullmac_driver
            .ifc_proxy
            .connect_conf(&fidl_fullmac::WlanFullmacImplIfcConnectConfRequest {
                peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
                result_code: Some(fidl_ieee80211::StatusCode::RefusedReasonUnspecified),
                association_id: Some(0),
                association_ies: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("Failed to send ConnectConf");

        connect_req_responder.send().expect("Failed to respond to connect req");

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::Deauth { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to Deauth");
        });
    };

    let (connect_result, _) = futures::join!(client_fut, driver_fut);
    assert_eq!(connect_result.code, fidl_ieee80211::StatusCode::RefusedReasonUnspecified);

    let fullmac_request_history = fullmac_driver.request_stream.history();

    // TODO(https://fxbug.dev/337074689): This is checked field by field because WEP key is initialized to some default
    // value determined by Banjo -> FIDL conversion code in wlanif. Instead of checking against
    // that default value, we ignore it in the test.
    let driver_connect_req =
        assert_matches!(&fullmac_request_history[0], FullmacRequest::Connect(req) => req.clone());
    assert_eq!(driver_connect_req.selected_bss.unwrap(), COMPATIBLE_OPEN_BSS.clone().into());
    assert_eq!(driver_connect_req.connect_failure_timeout.unwrap(), 60);
    assert_eq!(driver_connect_req.auth_type.unwrap(), fidl_fullmac::WlanAuthType::OpenSystem);

    // TODO(https://fxbug.dev/337074689): Check that these are None instead of empty vectors.
    assert_eq!(driver_connect_req.sae_password.unwrap(), vec![] as Vec<u8>);
    assert_eq!(driver_connect_req.security_ie.unwrap(), vec![] as Vec<u8>);

    assert_eq!(
        fullmac_request_history[1],
        FullmacRequest::Deauth(fidl_fullmac::WlanFullmacImplDeauthRequest {
            peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
            reason_code: Some(fidl_ieee80211::ReasonCode::StaLeaving),
            ..Default::default()
        })
    );
}

#[fuchsia::test]
async fn test_wpa2_connect_request_success() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

    let credentials = fidl_wlan_security::WpaCredentials::Passphrase(vec![8, 7, 6, 5, 4, 3, 2, 1]);

    let client_fut = async {
        let (connect_txn, connect_txn_server) =
            fidl::endpoints::create_proxy::<fidl_sme::ConnectTransactionMarker>();
        let mut connect_txn_event_stream = connect_txn.take_event_stream();

        let connect_req = fidl_sme::ConnectRequest {
            ssid: COMPATIBLE_WPA2_BSS.ssid.clone().into(),
            bss_description: COMPATIBLE_WPA2_BSS.clone().into(),
            multiple_bss_candidates: false,
            authentication: fidl_wlan_security::Authentication {
                protocol: fidl_wlan_security::Protocol::Wpa2Personal,
                credentials: Some(Box::new(fidl_wlan_security::Credentials::Wpa(
                    credentials.clone(),
                ))),
            },
            // Note: this field has no effect for fullmac drivers.
            deprecated_scan_type: fidl_common::ScanType::Passive,
        };

        client_sme_proxy
            .connect(&connect_req, Some(connect_txn_server))
            .expect("Connect FIDL error.");

        let connect_txn_event = connect_txn_event_stream
            .next()
            .await
            .expect("Connect event stream FIDL error")
            .expect("Connect txn returned error");

        // Returns the Connect result code.
        assert_matches!(connect_txn_event,
            fidl_sme::ConnectTransactionEvent::OnConnectResult { result } => {
                result
            }
        )
    };

    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::Connect { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to Connect");
        });

        fullmac_driver
            .ifc_proxy
            .connect_conf(&fidl_fullmac::WlanFullmacImplIfcConnectConfRequest {
                peer_sta_address: Some(COMPATIBLE_WPA2_BSS.bssid.to_array()),
                result_code: Some(fidl_ieee80211::StatusCode::Success),
                association_id: Some(0),
                association_ies: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("Failed to send ConnectConf");

        let mut authenticator = fullmac_helpers::fake_ap::create_wpa2_authenticator(
            MacAddr::from(fullmac_driver.sta_addr()),
            &COMPATIBLE_WPA2_BSS,
            credentials.clone(),
        );

        let initial_eapol_frame = {
            let mut update_sink = UpdateSink::new();
            authenticator.initiate(&mut update_sink).expect("Could not initiate authenticator");
            assert_matches!(
                update_sink.remove(0),
                SecAssocUpdate::Status(SecAssocStatus::PmkSaEstablished)
            );
            assert_matches!(update_sink.remove(0), SecAssocUpdate::TxEapolKeyFrame { frame, .. } => frame)
        };

        let update_sink = fullmac_helpers::fake_ap::handle_fourway_eapol_handshake(
            &mut authenticator,
            initial_eapol_frame,
            COMPATIBLE_WPA2_BSS.bssid.to_array(),
            fullmac_driver.sta_addr(),
            &mut fullmac_driver.request_stream,
            &fullmac_driver.ifc_proxy,
        )
        .await;

        // Expect PTK and GTK keys to be received by driver
        for _ in 0..2 {
            assert_matches!(fullmac_driver.request_stream.next().await,
                fidl_fullmac::WlanFullmacImpl_Request::SetKeys{ payload:_, responder } => {
                    responder.send(&fidl_fullmac::WlanFullmacSetKeysResp {
                        statuslist: vec![zx::sys::ZX_OK],
                    }).expect("Failed to respond to SetKeys");
            });
        }

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to OnLinkStateChanged");
        });

        update_sink
    };

    let (connect_result, auth_update_sink) = futures::join!(client_fut, driver_fut);
    assert_eq!(connect_result.code, fidl_ieee80211::StatusCode::Success);

    let fullmac_request_history = fullmac_driver.request_stream.history();

    // TODO(https://fxbug.dev/337074689): This is checked field by field because WEP key is initialized to some default
    // value determined by Banjo -> FIDL conversion code in wlanif. Instead of checking against
    // that default value, we ignore it in the test.
    let driver_connect_req =
        assert_matches!(&fullmac_request_history[0], FullmacRequest::Connect(req) => req.clone());
    assert_eq!(driver_connect_req.selected_bss.unwrap(), COMPATIBLE_WPA2_BSS.clone().into());
    assert_eq!(driver_connect_req.security_ie.unwrap(), COMPATIBLE_WPA2_BSS.rsne().unwrap());
    assert_eq!(driver_connect_req.connect_failure_timeout.unwrap(), 60);
    assert_eq!(driver_connect_req.auth_type.unwrap(), fidl_fullmac::WlanAuthType::OpenSystem);

    // TODO(https://fxbug.dev/337074689): Check that these are None instead of empty vectors.
    assert_eq!(driver_connect_req.sae_password.unwrap(), vec![] as Vec<u8>);

    let eapol_tx1 =
        assert_matches!(&fullmac_request_history[1], FullmacRequest::EapolTx(req) => req);
    assert_eq!(eapol_tx1.src_addr.unwrap(), fullmac_driver.sta_addr());
    assert_eq!(eapol_tx1.dst_addr.unwrap(), COMPATIBLE_WPA2_BSS.bssid.to_array());

    let eapol_tx2 =
        assert_matches!(&fullmac_request_history[2], FullmacRequest::EapolTx(req) => req);
    assert_eq!(eapol_tx2.src_addr.unwrap(), fullmac_driver.sta_addr());
    assert_eq!(eapol_tx2.dst_addr.unwrap(), COMPATIBLE_WPA2_BSS.bssid.to_array());

    // Check that PTK received by driver matches the authenticator's PTK
    let driver_ptk_req =
        assert_matches!(&fullmac_request_history[3], FullmacRequest::SetKeys(req) => req.clone());
    assert_eq!(driver_ptk_req.key_descriptors.as_ref().unwrap().len(), 1);
    let driver_ptk = driver_ptk_req.key_descriptors.as_ref().unwrap();
    let auth_ptk =
        assert_matches!(&auth_update_sink[0], SecAssocUpdate::Key(Key::Ptk(ptk)) => ptk.clone());

    assert_eq!(
        &driver_ptk[0],
        &fidl_ieee80211::SetKeyDescriptor {
            key_type: Some(fidl_ieee80211::KeyType::Pairwise),
            key_id: Some(0),
            peer_addr: Some(COMPATIBLE_WPA2_BSS.bssid.to_array()),
            cipher_type: Some(fidl_ieee80211::CipherSuiteType::Ccmp128),
            cipher_oui: Some(auth_ptk.cipher.oui.into()),
            key: Some(auth_ptk.tk().to_vec()),
            rsc: Some(0),
            ..Default::default()
        }
    );

    // Check that GTK received by driver matches the authenticator's GTK
    let driver_gtk_req =
        assert_matches!(&fullmac_request_history[4], FullmacRequest::SetKeys(req) => req);
    assert_eq!(driver_gtk_req.key_descriptors.as_ref().unwrap().len(), 1);
    let driver_gtk = driver_gtk_req.key_descriptors.clone().unwrap();
    let auth_gtk =
        assert_matches!(&auth_update_sink[1], SecAssocUpdate::Key(Key::Gtk(gtk)) => gtk.clone());

    assert_eq!(
        &driver_gtk[0],
        &fidl_ieee80211::SetKeyDescriptor {
            key_type: Some(fidl_ieee80211::KeyType::Group),
            key_id: Some(auth_gtk.key_id().into()),
            peer_addr: Some(ieee80211::BROADCAST_ADDR.to_array()),
            cipher_type: Some(fidl_ieee80211::CipherSuiteType::Ccmp128),
            cipher_oui: Some(auth_gtk.cipher().oui.into()),
            key: Some(auth_gtk.tk().to_vec()),
            rsc: Some(auth_gtk.key_rsc()),
            ..Default::default()
        }
    );

    assert_eq!(
        fullmac_request_history[5],
        FullmacRequest::OnLinkStateChanged(
            fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest {
                online: Some(true),
                ..Default::default()
            }
        )
    );
}

#[fuchsia::test]
async fn test_wpa3_connect_success() {
    let mut fullmac_driver =
        FullmacDriverFixture::create(FullmacDriverConfig { ..Default::default() }).await;
    let client_sme_proxy = sme_helpers::get_client_sme(&fullmac_driver.generic_sme_proxy).await;

    let credentials = fidl_wlan_security::WpaCredentials::Passphrase(vec![8, 7, 6, 5, 4, 3, 2, 1]);

    let client_fut = async {
        let (connect_txn, connect_txn_server) =
            fidl::endpoints::create_proxy::<fidl_sme::ConnectTransactionMarker>();
        let mut connect_txn_event_stream = connect_txn.take_event_stream();

        let connect_req = fidl_sme::ConnectRequest {
            ssid: COMPATIBLE_WPA3_BSS.ssid.clone().into(),
            bss_description: COMPATIBLE_WPA3_BSS.clone().into(),
            multiple_bss_candidates: false,
            authentication: fidl_wlan_security::Authentication {
                protocol: fidl_wlan_security::Protocol::Wpa3Personal,
                credentials: Some(Box::new(fidl_wlan_security::Credentials::Wpa(
                    credentials.clone(),
                ))),
            },
            // Note: this field has no effect for fullmac drivers.
            deprecated_scan_type: fidl_common::ScanType::Passive,
        };

        client_sme_proxy
            .connect(&connect_req, Some(connect_txn_server))
            .expect("Connect FIDL error.");

        let connect_txn_event = connect_txn_event_stream
            .next()
            .await
            .expect("Connect event stream FIDL error")
            .expect("Connect txn returned error");

        // Returns the Connect result code.
        assert_matches!(connect_txn_event,
            fidl_sme::ConnectTransactionEvent::OnConnectResult { result } => {
                result
            }
        )
    };

    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::Connect { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to Connect");
        });

        let mut authenticator = fullmac_helpers::fake_ap::create_wpa3_authenticator(
            MacAddr::from(fullmac_driver.sta_addr()),
            &COMPATIBLE_WPA3_BSS,
            credentials.clone(),
        );

        // Kick off SAE handshake by sending SaeHandshakeInd
        fullmac_driver
            .ifc_proxy
            .sae_handshake_ind(&fidl_fullmac::WlanFullmacImplIfcSaeHandshakeIndRequest {
                peer_sta_address: Some(COMPATIBLE_WPA3_BSS.bssid.to_array()),
                ..Default::default()
            })
            .await
            .expect("Could not send SaeHandshakeInd");

        // Note: SAE exchange must occur before sending ConnectConfirm
        let initial_eapol_frame = {
            let update_sink = fullmac_helpers::fake_ap::handle_sae_exchange(
                &mut authenticator,
                &mut fullmac_driver.request_stream,
                &fullmac_driver.ifc_proxy,
            )
            .await;

            assert!(update_sink.contains(&SecAssocUpdate::SaeAuthStatus(AuthStatus::Success)));
            assert!(
                update_sink.contains(&SecAssocUpdate::Status(SecAssocStatus::PmkSaEstablished))
            );

            // Get initial EAPOL frame from update_sink
            update_sink
                .iter()
                .find_map(|u| match u {
                    SecAssocUpdate::TxEapolKeyFrame { frame, .. } => Some(frame.clone()),
                    _ => None,
                })
                .unwrap()
        };

        let mut update_sink = UpdateSink::new();
        authenticator.initiate(&mut update_sink).expect("Could not initiate authenticator");
        assert_eq!(update_sink.len(), 0);

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::SaeHandshakeResp { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to SaeHandshakeResp");
        });

        fullmac_driver
            .ifc_proxy
            .connect_conf(&fidl_fullmac::WlanFullmacImplIfcConnectConfRequest {
                peer_sta_address: Some(COMPATIBLE_WPA3_BSS.bssid.to_array()),
                result_code: Some(fidl_ieee80211::StatusCode::Success),
                association_id: Some(0),
                association_ies: Some(vec![]),
                ..Default::default()
            })
            .await
            .expect("Failed to send ConnectConf");

        let update_sink = fullmac_helpers::fake_ap::handle_fourway_eapol_handshake(
            &mut authenticator,
            initial_eapol_frame,
            COMPATIBLE_WPA3_BSS.bssid.to_array(),
            fullmac_driver.sta_addr(),
            &mut fullmac_driver.request_stream,
            &fullmac_driver.ifc_proxy,
        )
        .await;

        // Expect PTK, GTK, and IGTK
        for _ in 0..3 {
            assert_matches!(fullmac_driver.request_stream.next().await,
                fidl_fullmac::WlanFullmacImpl_Request::SetKeys{ payload:_, responder } => {
                    responder.send(&fidl_fullmac::WlanFullmacSetKeysResp {
                        statuslist: vec![zx::sys::ZX_OK],
                    }).expect("Failed to respond to SetKeys");
            });
        }

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to OnLinkStateChanged");
        });

        update_sink
    };

    let (connect_result, auth_update_sink) = futures::join!(client_fut, driver_fut);
    assert_eq!(connect_result.code, fidl_ieee80211::StatusCode::Success);

    let fullmac_request_history = fullmac_driver.request_stream.history();

    // TODO(https://fxbug.dev/337074689): This is checked field by field because WEP key is initialized to some default
    // value determined by Banjo -> FIDL conversion code in wlanif. Instead of checking against
    // that default value, we ignore it in the test.
    let driver_connect_req =
        assert_matches!(&fullmac_request_history[0], FullmacRequest::Connect(req) => req.clone());
    assert_eq!(driver_connect_req.selected_bss.unwrap(), COMPATIBLE_WPA3_BSS.clone().into());
    assert_eq!(driver_connect_req.security_ie.unwrap(), COMPATIBLE_WPA3_BSS.rsne().unwrap());
    assert_eq!(driver_connect_req.connect_failure_timeout.unwrap(), 60);
    assert_eq!(driver_connect_req.auth_type.unwrap(), fidl_fullmac::WlanAuthType::Sae);

    // TODO(https://fxbug.dev/337074689): Check that these are None instead of empty vectors.
    assert_eq!(driver_connect_req.sae_password.unwrap(), vec![] as Vec<u8>);

    let sae_commit =
        assert_matches!(&fullmac_request_history[1], FullmacRequest::SaeFrameTx(req) => req);
    assert_eq!(sae_commit.peer_sta_address.unwrap(), COMPATIBLE_WPA3_BSS.bssid.to_array());
    assert_eq!(sae_commit.status_code.unwrap(), fidl_ieee80211::StatusCode::Success);
    assert_eq!(sae_commit.seq_num.unwrap(), 1);

    let sae_confirm =
        assert_matches!(&fullmac_request_history[2], FullmacRequest::SaeFrameTx(req) => req);
    assert_eq!(sae_confirm.peer_sta_address.unwrap(), COMPATIBLE_WPA3_BSS.bssid.to_array());
    assert_eq!(sae_confirm.status_code.unwrap(), fidl_ieee80211::StatusCode::Success);
    assert_eq!(sae_confirm.seq_num.unwrap(), 2);

    assert_eq!(
        fullmac_request_history[3],
        FullmacRequest::SaeHandshakeResp(fidl_fullmac::WlanFullmacImplSaeHandshakeRespRequest {
            peer_sta_address: Some(fullmac_driver.sta_addr()),
            status_code: Some(fidl_ieee80211::StatusCode::Success),
            ..Default::default()
        })
    );

    let eapol_tx1 =
        assert_matches!(&fullmac_request_history[4], FullmacRequest::EapolTx(req) => req);
    assert_eq!(eapol_tx1.src_addr.unwrap(), fullmac_driver.sta_addr());
    assert_eq!(eapol_tx1.dst_addr.unwrap(), COMPATIBLE_WPA2_BSS.bssid.to_array());

    let eapol_tx2 =
        assert_matches!(&fullmac_request_history[5], FullmacRequest::EapolTx(req) => req);
    assert_eq!(eapol_tx2.src_addr.unwrap(), fullmac_driver.sta_addr());
    assert_eq!(eapol_tx2.dst_addr.unwrap(), COMPATIBLE_WPA2_BSS.bssid.to_array());

    // Check that PTK received by driver matches the authenticator's PTK
    let driver_ptk_req =
        assert_matches!(&fullmac_request_history[6], FullmacRequest::SetKeys(req) => req.clone());
    assert_eq!(driver_ptk_req.key_descriptors.as_ref().unwrap().len(), 1);
    let driver_ptk = driver_ptk_req.key_descriptors.unwrap();
    let auth_ptk =
        assert_matches!(&auth_update_sink[0], SecAssocUpdate::Key(Key::Ptk(ptk)) => ptk.clone());

    assert_eq!(
        driver_ptk[0],
        fidl_ieee80211::SetKeyDescriptor {
            key_type: Some(fidl_ieee80211::KeyType::Pairwise),
            key_id: Some(0),
            peer_addr: Some(COMPATIBLE_WPA2_BSS.bssid.to_array()),
            cipher_type: Some(fidl_ieee80211::CipherSuiteType::Ccmp128),
            cipher_oui: Some(auth_ptk.cipher.oui.into()),
            key: Some(auth_ptk.tk().to_vec()),
            rsc: Some(0),
            ..Default::default()
        }
    );

    // Check that GTK received by driver matches the authenticator's GTK
    let driver_gtk_req =
        assert_matches!(&fullmac_request_history[7], FullmacRequest::SetKeys(req) => req);
    assert_eq!(driver_gtk_req.key_descriptors.as_ref().unwrap().len(), 1);
    let driver_gtk = &driver_gtk_req.key_descriptors.clone().unwrap();
    let auth_gtk =
        assert_matches!(&auth_update_sink[1], SecAssocUpdate::Key(Key::Gtk(gtk)) => gtk.clone());

    assert_eq!(
        driver_gtk[0],
        fidl_ieee80211::SetKeyDescriptor {
            key_type: Some(fidl_ieee80211::KeyType::Group),
            key_id: Some(auth_gtk.key_id().into()),
            peer_addr: Some(ieee80211::BROADCAST_ADDR.to_array()),
            cipher_type: Some(fidl_ieee80211::CipherSuiteType::Ccmp128),
            cipher_oui: Some(auth_gtk.cipher().oui.into()),
            key: Some(auth_gtk.tk().to_vec()),
            rsc: Some(auth_gtk.key_rsc()),
            ..Default::default()
        }
    );

    // Check that IGTK received by driver matches the authenticator's IGTK
    let driver_igtk_req =
        assert_matches!(&fullmac_request_history[8], FullmacRequest::SetKeys(req) => req);
    let driver_igtk = driver_igtk_req.key_descriptors.clone().unwrap();
    let auth_igtk =
        assert_matches!(&auth_update_sink[2], SecAssocUpdate::Key(Key::Igtk(igtk)) => igtk.clone());

    assert_eq!(
        driver_igtk[0],
        fidl_ieee80211::SetKeyDescriptor {
            key_type: Some(fidl_ieee80211::KeyType::Igtk),
            key_id: Some(auth_igtk.key_id.try_into().unwrap()),
            peer_addr: Some(ieee80211::BROADCAST_ADDR.to_array()),
            cipher_type: Some(fidl_ieee80211::CipherSuiteType::BipCmac128),
            cipher_oui: Some(auth_igtk.cipher.oui.into()),
            key: Some(auth_igtk.tk().to_vec()),
            rsc: Some(0),
            ..Default::default()
        }
    );

    assert_eq!(
        fullmac_request_history[9],
        FullmacRequest::OnLinkStateChanged(
            fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest {
                online: Some(true),
                ..Default::default()
            }
        )
    );
}

#[fuchsia::test]
async fn test_sme_disconnect() {
    let (client_sme_proxy, mut connect_txn_event_stream, mut fullmac_driver) =
        setup_connected_to_open_bss(FullmacDriverConfig { ..Default::default() }).await;

    let client_fut = client_sme_proxy
        .disconnect(fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest);

    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to OnLinkStateChanged");
        });

        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::Deauth { payload: _, responder } => {
                responder
                    .send()
                    .expect("Failed to respond to Deauth");
        });

        fullmac_driver
            .ifc_proxy
            .deauth_conf(&fidl_fullmac::WlanFullmacImplIfcDeauthConfRequest {
                peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
                ..Default::default()
            })
            .await
            .expect("Failed to send deauth conf");
    };

    let (_, _) = futures::join!(client_fut, driver_fut);

    let fullmac_request_history = fullmac_driver.request_stream.history();

    assert_eq!(
        fullmac_request_history[0],
        FullmacRequest::OnLinkStateChanged(
            fidl_fullmac::WlanFullmacImplOnLinkStateChangedRequest {
                online: Some(false),
                ..Default::default()
            }
        )
    );
    assert_eq!(
        fullmac_request_history[1],
        FullmacRequest::Deauth(fidl_fullmac::WlanFullmacImplDeauthRequest {
            peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
            reason_code: Some(fidl_ieee80211::ReasonCode::StaLeaving),
            ..Default::default()
        })
    );

    assert_matches!(
        connect_txn_event_stream.next().await,
        Some(Ok(fidl_sme::ConnectTransactionEvent::OnDisconnect { info })) => {
            assert!(!info.is_sme_reconnecting);
            assert_eq!(info.disconnect_source,
                fidl_sme::DisconnectSource::User(fidl_sme::UserDisconnectReason::FidlStopClientConnectionsRequest));
    });
}

#[fuchsia::test]
async fn test_remote_deauth() {
    let (_client_sme_proxy, mut connect_txn_event_stream, mut fullmac_driver) =
        setup_connected_to_open_bss(FullmacDriverConfig { ..Default::default() }).await;

    fullmac_driver
        .ifc_proxy
        .deauth_ind(&fidl_fullmac::WlanFullmacImplIfcDeauthIndRequest {
            peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
            reason_code: Some(fidl_ieee80211::ReasonCode::UnspecifiedReason),
            locally_initiated: Some(false),
            ..Default::default()
        })
        .await
        .expect("Could not send deauth ind");

    assert_matches!(fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload, responder } => {
          assert_eq!(payload.online, Some(false));
            responder
                .send()
                .expect("Failed to respond to OnLinkStateChanged");
    });

    assert_matches!(
        connect_txn_event_stream.next().await,
        Some(Ok(fidl_sme::ConnectTransactionEvent::OnDisconnect {
            info: fidl_sme::DisconnectInfo {
                is_sme_reconnecting: false,
                disconnect_source: fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
                    mlme_event_name: fidl_sme::DisconnectMlmeEventName::DeauthenticateIndication,
                    reason_code: fidl_ieee80211::ReasonCode::UnspecifiedReason
                }),
            }
        }))
    );
}

#[fuchsia::test]
async fn test_remote_disassoc_then_reconnect() {
    let (_client_sme_proxy, mut connect_txn_event_stream, mut fullmac_driver) =
        setup_connected_to_open_bss(FullmacDriverConfig { ..Default::default() }).await;

    fullmac_driver
        .ifc_proxy
        .disassoc_ind(&fidl_fullmac::WlanFullmacImplIfcDisassocIndRequest {
            peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
            reason_code: Some(fidl_ieee80211::ReasonCode::ReasonInactivity),
            locally_initiated: Some(false),
            ..Default::default()
        })
        .await
        .expect("Could not send DisassocInd");

    assert_matches!(
        fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload, responder } => {
          assert_eq!(payload.online, Some(false));
            responder
                .send()
                .expect("Failed to respond to OnLinkStateChanged");
    });

    assert_matches!(
        fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::Reconnect { payload, responder } => {
            responder
                .send()
                .expect("Failed to respond to Reconnect");
            assert_eq!(payload.peer_sta_address.unwrap(), COMPATIBLE_OPEN_BSS.bssid.to_array());
    });

    assert_matches!(
        connect_txn_event_stream.next().await,
        Some(Ok(fidl_sme::ConnectTransactionEvent::OnDisconnect {
            info: fidl_sme::DisconnectInfo {
                is_sme_reconnecting: true,
                disconnect_source: fidl_sme::DisconnectSource::Ap(fidl_sme::DisconnectCause {
                    mlme_event_name: fidl_sme::DisconnectMlmeEventName::DisassociateIndication,
                    reason_code: fidl_ieee80211::ReasonCode::ReasonInactivity,
                }),
            }
        }))
    );

    fullmac_driver
        .ifc_proxy
        .connect_conf(&fidl_fullmac::WlanFullmacImplIfcConnectConfRequest {
            peer_sta_address: Some(COMPATIBLE_OPEN_BSS.bssid.to_array()),
            result_code: Some(fidl_ieee80211::StatusCode::Success),
            association_id: Some(0),
            association_ies: Some(vec![]),
            ..Default::default()
        })
        .await
        .expect("Failed to send ConnectConf");

    assert_matches!(
        fullmac_driver.request_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::OnLinkStateChanged { payload, responder } => {
          assert_eq!(payload.online, Some(true));
            responder
                .send()
                .expect("Failed to respond to OnLinkStateChanged");
    });

    assert_matches!(
        connect_txn_event_stream.next().await,
        Some(Ok(fidl_sme::ConnectTransactionEvent::OnConnectResult {
            result: fidl_sme::ConnectResult {
                code: fidl_ieee80211::StatusCode::Success,
                is_credential_rejected: false,
                is_reconnect: true,
            }
        }))
    );
}

#[fuchsia::test]
async fn test_channel_switch() {
    let (_client_sme_proxy, mut connect_txn_event_stream, fullmac_driver) =
        setup_connected_to_open_bss(FullmacDriverConfig { ..Default::default() }).await;

    fullmac_driver
        .ifc_proxy
        .on_channel_switch(&fidl_fullmac::WlanFullmacChannelSwitchInfo {
            new_primary_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 11,
            },
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
        })
        .await
        .expect("Could not send OnChannelSwitch");

    assert_matches!(
        connect_txn_event_stream.next().await,
        Some(Ok(fidl_sme::ConnectTransactionEvent::OnChannelSwitched {
            info: fidl_internal::ChannelSwitchInfo {
                new_primary_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 11,
                },
                bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
                vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: 0,
                },
            }
        }))
    );
}

#[fuchsia::test]
async fn test_signal_report() {
    let (_client_sme_proxy, mut connect_txn_event_stream, fullmac_driver) =
        setup_connected_to_open_bss(FullmacDriverConfig { ..Default::default() }).await;

    for i in 0..10 {
        let expected_rssi_dbm = -40 + i;
        let expected_snr_db = 30 + i;
        let expected_tx_rate = 100 + i as u32;
        fullmac_driver
            .ifc_proxy
            .signal_report(&fidl_fullmac::WlanFullmacSignalReportIndication {
                rssi_dbm: expected_rssi_dbm,
                snr_db: expected_snr_db,
                tx_rate_500kbps: expected_tx_rate,
            })
            .await
            .expect("Could not send SignalReport");

        assert_matches!(
            connect_txn_event_stream.next().await,
            Some(Ok(fidl_sme::ConnectTransactionEvent::OnSignalReport {
                ind: fidl_internal::SignalReportIndication { rssi_dbm, snr_db, tx_rate_500kbps }
            })) => {
                assert_eq!(rssi_dbm, expected_rssi_dbm);
                assert_eq!(snr_db, expected_snr_db);
                assert_eq!(tx_rate_500kbps, expected_tx_rate);
            }
        );
    }
}
#[fuchsia::test]
async fn test_wmm_status() {
    let (client_sme_proxy, _connect_txn_event_stream, mut fullmac_driver) =
        setup_connected_to_open_bss(FullmacDriverConfig { ..Default::default() }).await;

    let gen_random_wmm_ac_params = || -> fidl_driver_common::WlanWmmAccessCategoryParameters {
        fidl_driver_common::WlanWmmAccessCategoryParameters {
            ecw_min: rand::random(),
            ecw_max: rand::random(),
            aifsn: rand::random(),
            txop_limit: rand::random(),
            acm: rand::random(),
        }
    };

    let wmm_params = fidl_driver_common::WlanWmmParameters {
        apsd: rand::random(),
        ac_be_params: gen_random_wmm_ac_params(),
        ac_bk_params: gen_random_wmm_ac_params(),
        ac_vi_params: gen_random_wmm_ac_params(),
        ac_vo_params: gen_random_wmm_ac_params(),
    };

    let client_fut = client_sme_proxy.wmm_status();
    let driver_fut = async {
        assert_matches!(fullmac_driver.request_stream.next().await,
            fidl_fullmac::WlanFullmacImpl_Request::WmmStatusReq { responder } => {
                responder
                    .send()
                    .expect("Failed to respond to WmmStatusReq");
        });

        fullmac_driver
            .ifc_proxy
            .on_wmm_status_resp(zx::sys::ZX_OK, &wmm_params)
            .await
            .expect("Could not send OnWmmStatusResp");
    };

    let (sme_wmm_status, _) = futures::join!(client_fut, driver_fut);
    let sme_wmm_status = sme_wmm_status
        .expect("FIDL error on WMM status")
        .expect("ClientSme returned error on WMM status");

    let wmm_ac_params_eq = |common: &fidl_driver_common::WlanWmmAccessCategoryParameters,
                            internal: &fidl_internal::WmmAcParams|
     -> bool {
        common.ecw_min == internal.ecw_min
            && common.ecw_max == internal.ecw_max
            && common.aifsn == internal.aifsn
            && common.txop_limit == internal.txop_limit
            && common.acm == internal.acm
    };

    assert_eq!(sme_wmm_status.apsd, wmm_params.apsd);
    assert!(wmm_ac_params_eq(&wmm_params.ac_be_params, &sme_wmm_status.ac_be_params));
    assert!(wmm_ac_params_eq(&wmm_params.ac_bk_params, &sme_wmm_status.ac_bk_params));
    assert!(wmm_ac_params_eq(&wmm_params.ac_vi_params, &sme_wmm_status.ac_vi_params));
    assert!(wmm_ac_params_eq(&wmm_params.ac_vo_params, &sme_wmm_status.ac_vo_params));
}
