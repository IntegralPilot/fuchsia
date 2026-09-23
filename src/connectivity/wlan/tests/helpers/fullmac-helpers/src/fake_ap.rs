// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::recorded_request_stream::RecordedRequestStream;
use assert_matches::assert_matches;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fidl_fuchsia_wlan_internal as fidl_wlan_security;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_mlme::EapolResultCode;
use fuchsia_sync::Mutex;
use ieee80211::MacAddr;
use std::sync::Arc;
use wlan_common::bss;
use wlan_common::ie::rsn::cipher::{CIPHER_BIP_CMAC_128, CIPHER_CCMP_128};
use wlan_common::ie::rsn::rsne;
use wlan_rsn::Authenticator;
use wlan_rsn::rsna::{SecAssocUpdate, UpdateSink};
use zerocopy::IntoBytes;

/// Creates a WPA2 authenticator based on the given parameters.
pub fn create_wpa2_authenticator(
    client_mac_addr: MacAddr,
    bss_description: &bss::BssDescription,
    credentials: fidl_wlan_security::WpaCredentials,
) -> Authenticator {
    assert_eq!(bss_description.protection(), bss::Protection::Wpa2Personal);

    // It's assumed that advertised and supplicant protections are the same.
    let advertised_protection_info = get_protection_info(bss_description);
    let supplicant_protection_info = get_protection_info(bss_description);

    let nonce_rdr = wlan_rsn::nonce::NonceReader::new(&bss_description.bssid.clone().into())
        .expect("creating nonce reader");
    let gtk_provider =
        wlan_rsn::GtkProvider::new(CIPHER_CCMP_128, 1, 0).expect("creating gtk provider");

    let psk = match credentials {
        fidl_wlan_security::WpaCredentials::Passphrase(passphrase) => {
            wlan_rsn::psk::compute(passphrase.as_bytes(), &bss_description.ssid)
                .expect("Could not compute psk")
        }
        fidl_wlan_security::WpaCredentials::Psk(psk) => Box::new(psk),
        _ => panic!("Unsupported credential type"),
    };

    Authenticator::new_wpa2psk_ccmp128(
        nonce_rdr,
        Arc::new(Mutex::new(gtk_provider)),
        psk,
        client_mac_addr,
        supplicant_protection_info,
        bss_description.bssid.clone().into(),
        advertised_protection_info,
    )
    .expect("Failed to create authenticator")
}

/// Creates a WPA3 authenticator based on the given parameters.
pub fn create_wpa3_authenticator(
    client_mac_addr: MacAddr,
    bss_description: &bss::BssDescription,
    credentials: fidl_wlan_security::WpaCredentials,
) -> Authenticator {
    assert_eq!(bss_description.protection(), bss::Protection::Wpa3Personal);

    // It's assumed that advertised and supplicant protections are the same.
    let advertised_protection_info = get_protection_info(bss_description);
    let supplicant_protection_info = get_protection_info(bss_description);

    let password =
        assert_matches!(credentials, fidl_wlan_security::WpaCredentials::Passphrase(p) => p);

    let nonce_rdr = wlan_rsn::nonce::NonceReader::new(&bss_description.bssid.clone().into())
        .expect("creating nonce reader");
    let gtk_provider =
        wlan_rsn::GtkProvider::new(CIPHER_CCMP_128, 1, 0).expect("creating gtk provider");
    let igtk_provider =
        wlan_rsn::IgtkProvider::new(CIPHER_BIP_CMAC_128).expect("error creating IgtkProvider");

    Authenticator::new_wpa3(
        nonce_rdr,
        Arc::new(Mutex::new(gtk_provider)),
        Arc::new(Mutex::new(igtk_provider)),
        bss_description.ssid.clone(),
        password,
        client_mac_addr,
        supplicant_protection_info.clone(),
        bss_description.bssid.into(),
        advertised_protection_info,
    )
    .expect("Failed to create authenticator")
}

// Uses |fullmac_req_stream| and |fullmac_ifc_proxy| to perform an SAE exchange as a WPA3
// authenticator.
//
// This assumes that the user has already sent an `SaeHandshakeInd` to the test realm, and the test
// realm is ready to send the first SAE commit frame to the authenticator.
//
// Returns the EAPOL key frame that will be used as the first frame in the EAPOL handshake.
// When this function returns, the user can expect that:
//  - |authenticator| is ready to be initiated and used in an EAPOL handshake.
//  - |fullmac_req_stream| has a pending `SaeHandshakeResp` request.
//
// Panics if the handshake fails for any reason.
pub async fn handle_sae_exchange(
    authenticator: &mut Authenticator,
    fullmac_req_stream: &mut RecordedRequestStream,
    fullmac_ifc_proxy: &fidl_fullmac::WlanFullmacImplIfcProxy,
) -> UpdateSink {
    let mut update_sink = UpdateSink::new();

    // Handle supplicant confirm
    let supplicant_commit_frame = get_sae_frame_from_test_realm(fullmac_req_stream).await;
    authenticator
        .on_sae_frame_rx(&mut update_sink, supplicant_commit_frame)
        .expect("Failed to send SAE commit frame to authenticator");

    // Authenticator produces the commit and confirm frame at the same time
    // after receiving the supplicant commit.
    let authenticator_commit_frame = assert_matches!(
        &update_sink[0], SecAssocUpdate::TxSaeFrame(frame) => frame.clone());
    let authenticator_confirm_frame = assert_matches!(
        &update_sink[1], SecAssocUpdate::TxSaeFrame(frame) => frame.clone());
    update_sink.clear();

    // Send the authenticator commit frame
    send_sae_frame_to_test_realm(authenticator_commit_frame, fullmac_ifc_proxy).await;

    // Handle supplicant confirm frame
    let supplicant_confirm_frame = get_sae_frame_from_test_realm(fullmac_req_stream).await;
    authenticator
        .on_sae_frame_rx(&mut update_sink, supplicant_confirm_frame)
        .expect("Failed to send SAE confirm frame to authenticator");

    // Send the authenticator confirm frame
    send_sae_frame_to_test_realm(authenticator_confirm_frame, fullmac_ifc_proxy).await;

    update_sink
}

/// Uses |fullmac_req_stream| and |fullmac_ifc_proxy| to perform an EAPOL handshake as an
/// authenticator.
/// This assumes that |authenticator| is already initiated.
/// |frame_to_client| is the first EAPOL frame that the authenticator sends.
///
/// Returns the UpdateSink of |authenticator|. By the end of a successful EAPOL handshake, the
/// UpdateSink should include all the keys for a successful EAPOL handshake.
/// Panics if the handshake fails for any reason.
pub async fn handle_fourway_eapol_handshake(
    authenticator: &mut Authenticator,
    frame_to_client: eapol::KeyFrameBuf,
    bssid: [u8; 6],
    client_sta_addr: [u8; 6],
    fullmac_req_stream: &mut RecordedRequestStream,
    fullmac_ifc_proxy: &fidl_fullmac::WlanFullmacImplIfcProxy,
) -> UpdateSink {
    let mut update_sink = UpdateSink::new();
    let mic_size = authenticator.get_negotiated_protection().mic_size;

    send_eapol_frame_to_test_realm(
        authenticator,
        frame_to_client,
        bssid.clone(),
        client_sta_addr.clone(),
        fullmac_ifc_proxy,
    )
    .await;
    let frame_to_auth_data =
        get_eapol_frame_from_test_realm(bssid.clone(), fullmac_req_stream, fullmac_ifc_proxy).await;
    let frame_to_auth = eapol::KeyFrameRx::parse(mic_size as usize, &frame_to_auth_data[..])
        .expect("Could not parse EAPOL key frame");
    authenticator
        .on_eapol_frame(&mut update_sink, eapol::Frame::Key(frame_to_auth))
        .expect("Could not send EAPOL frame to authenticator");

    let frame_to_client = assert_matches!(update_sink.remove(0), SecAssocUpdate::TxEapolKeyFrame { frame, .. } => frame);
    send_eapol_frame_to_test_realm(
        authenticator,
        frame_to_client,
        bssid.clone(),
        client_sta_addr.clone(),
        fullmac_ifc_proxy,
    )
    .await;
    let frame_to_auth_data =
        get_eapol_frame_from_test_realm(bssid.clone(), fullmac_req_stream, fullmac_ifc_proxy).await;
    let frame_to_auth = eapol::KeyFrameRx::parse(mic_size as usize, &frame_to_auth_data[..])
        .expect("Could not parse EAPOL key frame");
    authenticator
        .on_eapol_frame(&mut update_sink, eapol::Frame::Key(frame_to_auth))
        .expect("Could not send EAPOL frame to authenticator");

    update_sink
}

/// Sends an EAPOL |frame| to the test realm via |fullmac_ifc_proxy|.
async fn send_eapol_frame_to_test_realm(
    authenticator: &mut Authenticator,
    frame: eapol::KeyFrameBuf,
    authenticator_addr: [u8; 6],
    client_addr: [u8; 6],
    fullmac_ifc_proxy: &fidl_fullmac::WlanFullmacImplIfcProxy,
) {
    fullmac_ifc_proxy
        .eapol_ind(&fidl_fullmac::WlanFullmacImplIfcEapolIndRequest {
            src_addr: Some(authenticator_addr),
            dst_addr: Some(client_addr),
            data: Some(frame.into()),
            ..Default::default()
        })
        .await
        .expect("Could not send EAPOL ind");
    let mut update_sink = UpdateSink::new();
    authenticator
        .on_eapol_conf(&mut update_sink, EapolResultCode::Success)
        .expect("Could not send EAPOL conf to authenticator");
    assert_eq!(update_sink.len(), 0);
}

/// Returns the buffer containing EAPOL frame data from the test realm through |fullmac_req_stream|.
async fn get_eapol_frame_from_test_realm(
    authenticator_addr: [u8; 6],
    fullmac_req_stream: &mut RecordedRequestStream,
    fullmac_ifc_proxy: &fidl_fullmac::WlanFullmacImplIfcProxy,
) -> Vec<u8> {
    let frame_data = assert_matches!(fullmac_req_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::EapolTx { payload, responder } => {
            responder
                .send()
                .expect("Failed to respond to EapolTx");
            payload.data.unwrap()
    });

    fullmac_ifc_proxy
        .eapol_conf(&fidl_fullmac::WlanFullmacImplIfcEapolConfRequest {
            result_code: Some(fidl_fullmac::EapolTxResult::Success),
            dst_addr: Some(authenticator_addr),
            ..Default::default()
        })
        .await
        .expect("Could not send EAPOL conf");

    frame_data
}

fn get_protection_info(bss_description: &bss::BssDescription) -> wlan_rsn::ProtectionInfo {
    let (_, rsne) = rsne::from_bytes(bss_description.rsne().unwrap()).expect("Could not get RSNE");
    wlan_rsn::ProtectionInfo::Rsne(rsne)
}

async fn get_sae_frame_from_test_realm(
    fullmac_req_stream: &mut RecordedRequestStream,
) -> fidl_mlme::SaeFrame {
    let fullmac_sae_frame = assert_matches!(fullmac_req_stream.next().await,
        fidl_fullmac::WlanFullmacImpl_Request::SaeFrameTx { frame, responder } => {
            responder
                .send()
                .expect("Failed to respond to SaeFrameTx");
            frame
    });

    fidl_mlme::SaeFrame {
        peer_sta_address: fullmac_sae_frame.peer_sta_address.unwrap(),
        status_code: fullmac_sae_frame.status_code.unwrap(),
        seq_num: fullmac_sae_frame.seq_num.unwrap(),
        sae_fields: fullmac_sae_frame.sae_fields.unwrap(),
    }
}

async fn send_sae_frame_to_test_realm(
    frame: fidl_mlme::SaeFrame,
    fullmac_ifc_proxy: &fidl_fullmac::WlanFullmacImplIfcProxy,
) {
    fullmac_ifc_proxy
        .sae_frame_rx(&fidl_fullmac::SaeFrame {
            peer_sta_address: Some(frame.peer_sta_address),
            status_code: Some(frame.status_code),
            seq_num: Some(frame.seq_num),
            sae_fields: Some(frame.sae_fields.clone()),
            ..Default::default()
        })
        .await
        .expect("Could not send authenticator SAE commit frame");
}
