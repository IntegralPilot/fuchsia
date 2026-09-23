// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Error, Result, bail};
use fidl_fuchsia_wlan_driver as fidl_driver_common;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_internal as fidl_internal;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use log::warn;

pub fn convert_device_info(
    info: fidl_fullmac::WlanFullmacImplQueryResponse,
) -> Result<fidl_mlme::DeviceInfo> {
    let bands: Vec<fidl_mlme::BandCapability> = info
        .band_caps
        .context("missing band_caps")?
        .into_iter()
        .map(|band_cap| convert_band_cap(band_cap))
        .collect::<Result<Vec<fidl_mlme::BandCapability>>>()
        .context("could not convert band_cap")?;
    Ok(fidl_mlme::DeviceInfo {
        sta_addr: info.sta_addr.context("missing sta_addr")?,
        factory_addr: info.factory_addr.context("missing factory_addr")?,
        role: info.role.context("missing role")?,
        bands,
        // TODO(https://fxbug.dev/42169534): This field will be replaced in the new driver features
        // framework.
        softmac_hardware_capability: 0,
        // TODO(https://fxbug.dev/42120297): This field is stubbed out for future use.
        qos_capable: false,
    })
}

pub fn convert_set_keys_resp(
    resp: fidl_fullmac::WlanFullmacSetKeysResp,
    original_set_keys_req: &fidl_mlme::SetKeysRequest,
) -> Result<fidl_mlme::SetKeysConfirm> {
    if resp.statuslist.len() != original_set_keys_req.keylist.len() {
        bail!(
            "SetKeysReq and SetKeysResp num_keys count differ: {} != {}",
            original_set_keys_req.keylist.len(),
            resp.statuslist.len()
        );
    }
    let mut results = vec![];
    for i in 0..resp.statuslist.len() {
        // Okay to index because `i` is less than the length of `resp.statuslist`, and we
        // already checked that `keylist` is the same length as `resp.statuslist`
        #[expect(clippy::indexing_slicing)]
        results.push(fidl_mlme::SetKeyResult {
            key_id: original_set_keys_req.keylist[i].key_id,
            status: resp.statuslist[i],
        });
    }
    Ok(fidl_mlme::SetKeysConfirm { results })
}

pub fn convert_scan_result(
    result: fidl_fullmac::WlanFullmacImplIfcOnScanResultRequest,
) -> Result<fidl_mlme::ScanResult> {
    Ok(fidl_mlme::ScanResult {
        txn_id: result.txn_id.context("missing txn_id")?,
        timestamp_nanos: result.timestamp_nanos.context("missing timestamp_nanos")?,
        bss: result.bss.context("missing bss")?,
    })
}

pub fn convert_scan_end(
    end: fidl_fullmac::WlanFullmacImplIfcOnScanEndRequest,
) -> Result<fidl_mlme::ScanEnd> {
    use fidl_fullmac::WlanScanResult;
    let scan_result_code = end.code.context("missing code")?;
    Ok(fidl_mlme::ScanEnd {
        txn_id: end.txn_id.context("missing txn_id")?,
        code: match scan_result_code {
            WlanScanResult::Success => fidl_mlme::ScanResultCode::Success,
            WlanScanResult::NotSupported => fidl_mlme::ScanResultCode::NotSupported,
            WlanScanResult::InvalidArgs => fidl_mlme::ScanResultCode::InvalidArgs,
            WlanScanResult::InternalError => fidl_mlme::ScanResultCode::InternalError,
            WlanScanResult::ShouldWait => fidl_mlme::ScanResultCode::ShouldWait,
            WlanScanResult::CanceledByDriverOrFirmware => {
                fidl_mlme::ScanResultCode::CanceledByDriverOrFirmware
            }
            _ => {
                warn!(
                    "Invalid scan result code {}, defaulting to ScanResultCode::NotSupported",
                    scan_result_code.into_primitive()
                );
                fidl_mlme::ScanResultCode::NotSupported
            }
        },
    })
}

pub fn convert_connect_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcConnectConfRequest,
) -> Result<fidl_mlme::ConnectConfirm> {
    Ok(fidl_mlme::ConnectConfirm {
        peer_sta_address: conf.peer_sta_address.context("missing peer_sta_address")?,
        result_code: conf.result_code.context("missing result_code")?,
        association_id: if conf.result_code == Some(fidl_ieee80211::StatusCode::Success) {
            conf.association_id.context("missing association_id")?
        } else {
            0
        },
        association_ies: if conf.result_code == Some(fidl_ieee80211::StatusCode::Success) {
            conf.association_ies.context("missing association_ies")?
        } else {
            vec![]
        },
    })
}

pub fn convert_roam_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcRoamConfRequest,
) -> Result<fidl_mlme::RoamConfirm> {
    match conf.status_code {
        Some(status_code) => match status_code {
            fidl_ieee80211::StatusCode::Success => Ok(fidl_mlme::RoamConfirm {
                selected_bssid: conf.selected_bssid.context("missing selected BSSID")?,
                status_code,
                original_association_maintained: conf
                    .original_association_maintained
                    .context("missing original_association_maintained")?,
                target_bss_authenticated: conf
                    .target_bss_authenticated
                    .context("missing target_bss_authenticated")?,
                association_id: conf.association_id.context("missing association_id")?,
                association_ies: conf.association_ies.context("missing association_ies")?,
            }),
            _ => Ok(fidl_mlme::RoamConfirm {
                selected_bssid: conf.selected_bssid.context("missing selected BSSID")?,
                status_code,
                original_association_maintained: conf
                    .original_association_maintained
                    .context("missing original_association_maintained")?,
                target_bss_authenticated: conf
                    .target_bss_authenticated
                    .context("missing target_bss_authenticated")?,
                association_id: 0,
                association_ies: Vec::new(),
            }),
        },
        None => Err(Error::msg("Fullmac RoamConf is missing status_code")),
    }
}

pub fn convert_roam_start_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcRoamStartIndRequest,
) -> Result<fidl_mlme::RoamStartIndication> {
    Ok(fidl_mlme::RoamStartIndication {
        selected_bss: ind.selected_bss.context("missing selected_bss")?,
        selected_bssid: ind.selected_bssid.context("missing selected bssid")?,
        original_association_maintained: ind
            .original_association_maintained
            .context("missing original_association_maintained")?,
    })
}

pub fn convert_roam_result_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcRoamResultIndRequest,
) -> Result<fidl_mlme::RoamResultIndication> {
    Ok(fidl_mlme::RoamResultIndication {
        selected_bssid: ind
            .selected_bssid
            .context("malformed roam result indication: missing selected_bssid")?,
        status_code: ind
            .status_code
            .context("malformed roam result indication: missing status_code")?,
        original_association_maintained: ind
            .original_association_maintained
            .context("malformed roam result indication: missing original_association_maintained")?,
        target_bss_authenticated: ind
            .target_bss_authenticated
            .context("malformed roam result indication: missing target_bss_authenticated")?,
        association_id: ind
            .association_id
            .context("malformed roam result indication: missing association_id")?,
        association_ies: ind
            .association_ies
            .context("malformed roam result indication: missing association_ies")?,
    })
}

pub fn convert_authenticate_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcAuthIndRequest,
) -> Result<fidl_mlme::AuthenticateIndication> {
    use fidl_fullmac::WlanAuthType;
    let auth_type = ind.auth_type.context("missing auth type")?;
    Ok(fidl_mlme::AuthenticateIndication {
        peer_sta_address: ind.peer_sta_address.context("missing peer_sta_address")?,
        auth_type: match auth_type {
            WlanAuthType::OpenSystem => fidl_mlme::AuthenticationTypes::OpenSystem,
            WlanAuthType::SharedKey => fidl_mlme::AuthenticationTypes::SharedKey,
            WlanAuthType::FastBssTransition => fidl_mlme::AuthenticationTypes::FastBssTransition,
            WlanAuthType::Sae => fidl_mlme::AuthenticationTypes::Sae,
            _ => {
                warn!(
                    "Invalid auth type {}, defaulting to AuthenticationTypes::OpenSystem",
                    auth_type.into_primitive()
                );
                fidl_mlme::AuthenticationTypes::OpenSystem
            }
        },
    })
}

pub fn convert_deauthenticate_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcDeauthConfRequest,
) -> fidl_mlme::DeauthenticateConfirm {
    let peer_sta_address = conf.peer_sta_address.unwrap_or_else(|| {
        warn!("Got None for peer_sta_address when converting DeauthConf. Substituting all zeros.");
        [0 as u8; fidl_ieee80211::MAC_ADDR_LEN as usize]
    });
    fidl_mlme::DeauthenticateConfirm { peer_sta_address }
}

pub fn convert_deauthenticate_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcDeauthIndRequest,
) -> Result<fidl_mlme::DeauthenticateIndication> {
    Ok(fidl_mlme::DeauthenticateIndication {
        peer_sta_address: ind.peer_sta_address.context("missing peer sta address")?,
        reason_code: ind.reason_code.context("missing reason code")?,
        locally_initiated: ind.locally_initiated.context("missing locally initiated")?,
    })
}
pub fn convert_associate_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcAssocIndRequest,
) -> Result<fidl_mlme::AssociateIndication> {
    let ssid = ind.ssid.context("missing ssid")?;
    let rsne = ind.rsne.context("missing rsne")?;
    Ok(fidl_mlme::AssociateIndication {
        peer_sta_address: ind.peer_sta_address.context("missing peer sta address")?,
        // TODO(https://fxbug.dev/42068281): Fix the discrepancy between WlanFullmacAssocInd and
        // fidl_mlme::AssociateIndication
        capability_info: 0,
        listen_interval: ind.listen_interval.context("missing listen interval")?,
        ssid: if ssid.len() > 0 { Some(ssid) } else { None },
        rates: vec![],
        rsne: if rsne.len() > 0 { Some(rsne) } else { None },
    })
}

pub fn convert_disassociate_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcDisassocConfRequest,
) -> fidl_mlme::DisassociateConfirm {
    let status = conf.status.unwrap_or_else(|| {
        warn!("Got None for status when converting DisassocConf. Using error INTERNAL.");
        zx::Status::INTERNAL.into_raw()
    });
    fidl_mlme::DisassociateConfirm { status }
}

pub fn convert_disassociate_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcDisassocIndRequest,
) -> Result<fidl_mlme::DisassociateIndication> {
    Ok(fidl_mlme::DisassociateIndication {
        peer_sta_address: ind.peer_sta_address.context("missing peer_sta_address")?,
        reason_code: ind.reason_code.context("missing reason_code")?,
        locally_initiated: ind.locally_initiated.context("missing locally_initiated")?,
    })
}

pub fn convert_start_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcStartConfRequest,
) -> Result<fidl_mlme::StartConfirm> {
    use fidl_fullmac::StartResult;
    let result_code = conf.result_code.context("missing result_code")?;
    Ok(fidl_mlme::StartConfirm {
        result_code: match result_code {
            StartResult::Success => fidl_mlme::StartResultCode::Success,
            StartResult::BssAlreadyStartedOrJoined => {
                fidl_mlme::StartResultCode::BssAlreadyStartedOrJoined
            }
            StartResult::ResetRequiredBeforeStart => {
                fidl_mlme::StartResultCode::ResetRequiredBeforeStart
            }
            StartResult::NotSupported => fidl_mlme::StartResultCode::NotSupported,
            _ => {
                warn!(
                    "Invalid start result {}, defaulting to StartResultCode::InternalError",
                    result_code.into_primitive()
                );
                fidl_mlme::StartResultCode::InternalError
            }
        },
    })
}
pub fn convert_stop_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcStopConfRequest,
) -> Result<fidl_mlme::StopConfirm> {
    use fidl_fullmac::StopResult;
    let result_code = conf.result_code.context("missing result_code")?;
    Ok(fidl_mlme::StopConfirm {
        result_code: match result_code {
            StopResult::Success => fidl_mlme::StopResultCode::Success,
            StopResult::BssAlreadyStopped => fidl_mlme::StopResultCode::BssAlreadyStopped,
            StopResult::InternalError => fidl_mlme::StopResultCode::InternalError,
            _ => {
                warn!(
                    "Invalid stop result {}, defaulting to StopResultCode::InternalError",
                    result_code.into_primitive()
                );
                fidl_mlme::StopResultCode::InternalError
            }
        },
    })
}
pub fn convert_eapol_confirm(
    conf: fidl_fullmac::WlanFullmacImplIfcEapolConfRequest,
) -> Result<fidl_mlme::EapolConfirm> {
    use fidl_fullmac::EapolTxResult;
    let result_code = conf.result_code.context("missing result_code")?;
    Ok(fidl_mlme::EapolConfirm {
        result_code: match result_code {
            EapolTxResult::Success => fidl_mlme::EapolResultCode::Success,
            EapolTxResult::TransmissionFailure => fidl_mlme::EapolResultCode::TransmissionFailure,
            _ => {
                warn!(
                    "Invalid eapol result code {}, defaulting to EapolResultCode::TransmissionFailure",
                    result_code.into_primitive()
                );
                fidl_mlme::EapolResultCode::TransmissionFailure
            }
        },
        dst_addr: conf.dst_addr.context("missing dst_addr")?,
    })
}
pub fn convert_channel_switch_info(
    info: fidl_fullmac::WlanFullmacChannelSwitchInfo,
) -> fidl_internal::ChannelSwitchInfo {
    fidl_internal::ChannelSwitchInfo {
        new_primary_channel: info.new_primary_channel,
        bandwidth: info.bandwidth,
        vht_secondary_80_channel: info.vht_secondary_80_channel,
    }
}
pub fn convert_signal_report_indication(
    ind: fidl_fullmac::WlanFullmacSignalReportIndication,
) -> fidl_internal::SignalReportIndication {
    fidl_internal::SignalReportIndication {
        rssi_dbm: ind.rssi_dbm,
        snr_db: ind.snr_db,
        tx_rate_500kbps: ind.tx_rate_500kbps,
    }
}
pub fn convert_eapol_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcEapolIndRequest,
) -> Result<fidl_mlme::EapolIndication> {
    Ok(fidl_mlme::EapolIndication {
        src_addr: ind.src_addr.context("missing src_addr")?,
        dst_addr: ind.dst_addr.context("missing dst_addr")?,
        data: ind.data.context("missing data")?,
    })
}
pub fn convert_pmk_info(
    info: fidl_fullmac::WlanFullmacImplIfcOnPmkAvailableRequest,
) -> Result<fidl_mlme::PmkInfo> {
    Ok(fidl_mlme::PmkInfo {
        pmk: info.pmk.context("missing pmk")?,
        pmkid: info.pmkid.context("missing pmkid")?,
    })
}
pub fn convert_sae_handshake_indication(
    ind: fidl_fullmac::WlanFullmacImplIfcSaeHandshakeIndRequest,
) -> Result<fidl_mlme::SaeHandshakeIndication> {
    Ok(fidl_mlme::SaeHandshakeIndication {
        peer_sta_address: ind.peer_sta_address.context("missing peer_sta_address")?,
    })
}
pub fn convert_sae_frame(frame: fidl_fullmac::SaeFrame) -> Result<fidl_mlme::SaeFrame> {
    Ok(fidl_mlme::SaeFrame {
        peer_sta_address: frame.peer_sta_address.context("missing peer_sta_address")?,
        status_code: frame.status_code.context("missing status code")?,
        seq_num: frame.seq_num.context("missing seq_num")?,
        sae_fields: frame.sae_fields.context("missing sae_fields")?,
    })
}
pub fn convert_wmm_params(
    wmm_params: fidl_driver_common::WlanWmmParameters,
) -> fidl_internal::WmmStatusResponse {
    fidl_internal::WmmStatusResponse {
        apsd: wmm_params.apsd,
        ac_be_params: convert_wmm_ac_params(wmm_params.ac_be_params),
        ac_bk_params: convert_wmm_ac_params(wmm_params.ac_bk_params),
        ac_vi_params: convert_wmm_ac_params(wmm_params.ac_vi_params),
        ac_vo_params: convert_wmm_ac_params(wmm_params.ac_vo_params),
    }
}
fn convert_wmm_ac_params(
    params: fidl_driver_common::WlanWmmAccessCategoryParameters,
) -> fidl_internal::WmmAcParams {
    fidl_internal::WmmAcParams {
        ecw_min: params.ecw_min,
        ecw_max: params.ecw_max,
        aifsn: params.aifsn,
        txop_limit: params.txop_limit,
        acm: params.acm,
    }
}

fn convert_band_cap(cap: fidl_fullmac::BandCapability) -> Result<fidl_mlme::BandCapability> {
    Ok(fidl_mlme::BandCapability {
        band: cap.band.context("missing band")?,
        basic_rates: cap.basic_rates.context("missing basic_rates")?,
        ht_cap: cap.ht_caps.map(Box::new),
        vht_cap: cap.vht_caps.map(Box::new),
        primary_channels: cap.primary_channels.context("missing primary_channels")?,
    })
}

pub fn convert_read_apf_packet_filter_data_resp(
    resp: fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse,
) -> Result<fidl_mlme::MlmeReadApfPacketFilterDataResponse> {
    Ok(fidl_mlme::MlmeReadApfPacketFilterDataResponse {
        memory: resp.memory.context("missing memory")?,
    })
}

pub fn convert_get_apf_packet_filter_enabled_resp(
    resp: fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse,
) -> Result<fidl_mlme::MlmeGetApfPacketFilterEnabledResponse> {
    Ok(fidl_mlme::MlmeGetApfPacketFilterEnabledResponse {
        enabled: resp.enabled.context("missing enabled")?,
    })
}

pub fn convert_get_scheduled_scan_enabled_resp(
    resp: fidl_fullmac::WlanFullmacImplGetScheduledScanEnabledResponse,
) -> Result<fidl_mlme::MlmeGetScheduledScanEnabledResponse> {
    let active_txn_ids = resp.active_txn_ids.context("missing active_txn_ids")?;
    Ok(fidl_mlme::MlmeGetScheduledScanEnabledResponse { active_txn_ids })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_set_key_descriptor() -> fidl_mlme::SetKeyDescriptor {
        fidl_mlme::SetKeyDescriptor {
            key: vec![99, 100, 101, 102, 103, 14],
            key_id: 23,
            key_type: fidl_mlme::KeyType::Group,
            address: [4u8; 6],
            rsc: 123456,
            cipher_suite_oui: [77, 88, 99],
            cipher_suite_type: fidl_ieee80211::CipherSuiteType::Ccmp128,
        }
    }

    #[test]
    fn test_convert_set_keys_resp() {
        let fullmac_resp =
            fidl_fullmac::WlanFullmacSetKeysResp { statuslist: vec![zx::sys::ZX_ERR_INTERNAL; 1] };
        let original_req =
            fidl_mlme::SetKeysRequest { keylist: vec![fake_set_key_descriptor(); 1] };

        assert_eq!(
            convert_set_keys_resp(fullmac_resp, &original_req).unwrap(),
            fidl_mlme::SetKeysConfirm {
                results: vec![fidl_mlme::SetKeyResult {
                    key_id: original_req.keylist[0].key_id,
                    status: zx::sys::ZX_ERR_INTERNAL,
                }],
            }
        );
    }

    #[test]
    fn test_convert_set_keys_resp_mismatching_original_req_is_error() {
        let fullmac_resp =
            fidl_fullmac::WlanFullmacSetKeysResp { statuslist: vec![zx::sys::ZX_ERR_INTERNAL; 2] };
        let original_req =
            fidl_mlme::SetKeysRequest { keylist: vec![fake_set_key_descriptor(); 1] };
        assert!(convert_set_keys_resp(fullmac_resp, &original_req).is_err());
    }

    #[test]
    fn test_convert_authenticate_indication_with_unknown_auth_type_defaults_to_open_system() {
        let fullmac = fidl_fullmac::WlanFullmacImplIfcAuthIndRequest {
            peer_sta_address: Some([8; 6]),
            auth_type: Some(fidl_fullmac::WlanAuthType::from_primitive_allow_unknown(100)),
            ..Default::default()
        };
        assert_eq!(
            convert_authenticate_indication(fullmac).unwrap(),
            fidl_mlme::AuthenticateIndication {
                peer_sta_address: [8; 6],
                auth_type: fidl_mlme::AuthenticationTypes::OpenSystem,
            }
        );
    }

    #[test]
    fn test_convert_deauthenticate_confirm_missing_address_defaults_to_zeros() {
        let fullmac = fidl_fullmac::WlanFullmacImplIfcDeauthConfRequest {
            peer_sta_address: None,
            ..Default::default()
        };
        assert_eq!(
            convert_deauthenticate_confirm(fullmac),
            fidl_mlme::DeauthenticateConfirm { peer_sta_address: [0; 6] }
        );
    }

    #[test]
    fn test_convert_associate_indication_empty_vec_and_ssid_are_none() {
        let fullmac = fidl_fullmac::WlanFullmacImplIfcAssocIndRequest {
            peer_sta_address: Some([3; 6]),
            listen_interval: Some(123),
            ssid: vec![].into(),
            rsne: vec![].into(),
            vendor_ie: vec![].into(),
            ..Default::default()
        };

        let mlme = convert_associate_indication(fullmac).unwrap();
        assert!(mlme.ssid.is_none());
        assert!(mlme.rsne.is_none());
    }

    #[test]
    fn test_convert_start_confirm_unknown_result_code_defaults_to_internal_error() {
        let fullmac = fidl_fullmac::WlanFullmacImplIfcStartConfRequest {
            result_code: Some(fidl_fullmac::StartResult::from_primitive_allow_unknown(123)),
            ..Default::default()
        };
        assert_eq!(
            convert_start_confirm(fullmac).unwrap(),
            fidl_mlme::StartConfirm { result_code: fidl_mlme::StartResultCode::InternalError }
        );
    }

    #[test]
    fn test_convert_stop_confirm_unknown_result_code_defaults_to_internal_error() {
        let fullmac = fidl_fullmac::WlanFullmacImplIfcStopConfRequest {
            result_code: Some(fidl_fullmac::StopResult::from_primitive_allow_unknown(123)),
            ..Default::default()
        };
        assert_eq!(
            convert_stop_confirm(fullmac).unwrap(),
            fidl_mlme::StopConfirm { result_code: fidl_mlme::StopResultCode::InternalError }
        );
    }

    #[test]
    fn test_convert_eapol_confirm_unknown_result_code_defaults_to_transmission_failure() {
        let fullmac = fidl_fullmac::WlanFullmacImplIfcEapolConfRequest {
            dst_addr: Some([1; 6]),
            result_code: Some(fidl_fullmac::EapolTxResult::from_primitive_allow_unknown(123)),
            ..Default::default()
        };
        assert_eq!(
            convert_eapol_confirm(fullmac).unwrap(),
            fidl_mlme::EapolConfirm {
                dst_addr: [1; 6],
                result_code: fidl_mlme::EapolResultCode::TransmissionFailure,
            }
        );
    }

    //
    // Tests for helper functions
    //
    #[test]
    fn test_convert_band_cap() {
        let fullmac = fidl_fullmac::BandCapability {
            band: Some(fidl_ieee80211::WlanBand::FiveGhz),
            basic_rates: Some(vec![123; 3]),
            ht_caps: Some(fidl_ieee80211::HtCapabilities { bytes: [8; 26] }),
            vht_caps: Some(fidl_ieee80211::VhtCapabilities { bytes: [9; 12] }),
            primary_channels: Some(vec![
                fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::FiveGhz,
                    number: 21,
                };
                45
            ]),
            ..Default::default()
        };

        assert_eq!(
            convert_band_cap(fullmac).unwrap(),
            fidl_mlme::BandCapability {
                band: fidl_ieee80211::WlanBand::FiveGhz,
                basic_rates: vec![123; 3],
                ht_cap: Some(Box::new(fidl_ieee80211::HtCapabilities { bytes: [8; 26] })),
                vht_cap: Some(Box::new(fidl_ieee80211::VhtCapabilities { bytes: [9; 12] })),
                primary_channels: vec![
                    fidl_ieee80211::ChannelNumber {
                        band: fidl_ieee80211::WlanBand::FiveGhz,
                        number: 21,
                    };
                    45
                ],
            }
        );
    }

    #[test]
    fn test_convert_band_cap_no_ht_vht_become_none() {
        let fullmac = fidl_fullmac::BandCapability {
            band: Some(fidl_ieee80211::WlanBand::FiveGhz),
            basic_rates: Some(vec![123; 3]),
            ht_caps: None,
            vht_caps: None,
            primary_channels: Some(vec![
                fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::FiveGhz,
                    number: 21,
                };
                45
            ]),
            ..Default::default()
        };

        let mlme = convert_band_cap(fullmac).unwrap();
        assert!(mlme.ht_cap.is_none());
        assert!(mlme.vht_cap.is_none());
    }

    #[test]
    fn test_convert_read_apf_packet_filter_data_resp() {
        let fullmac_resp = fidl_fullmac::WlanFullmacImplReadApfPacketFilterDataResponse {
            memory: Some(vec![1, 2, 3, 4]),
            ..Default::default()
        };
        assert_eq!(
            convert_read_apf_packet_filter_data_resp(fullmac_resp).unwrap(),
            fidl_mlme::MlmeReadApfPacketFilterDataResponse { memory: vec![1, 2, 3, 4] }
        );
    }

    #[test]
    fn test_convert_get_apf_packet_filter_enabled_resp() {
        let fullmac_resp = fidl_fullmac::WlanFullmacImplGetApfPacketFilterEnabledResponse {
            enabled: Some(true),
            ..Default::default()
        };
        assert_eq!(
            convert_get_apf_packet_filter_enabled_resp(fullmac_resp).unwrap(),
            fidl_mlme::MlmeGetApfPacketFilterEnabledResponse { enabled: true }
        );
    }
}
