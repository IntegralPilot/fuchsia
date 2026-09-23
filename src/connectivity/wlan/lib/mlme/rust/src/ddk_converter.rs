// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::format_err;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_mlme as fidl_mlme;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use std::fmt::Display;

#[macro_export]
macro_rules! zeroed_array_from_prefix {
    ($slice:expr, $size:expr $(,)?) => {{
        assert!($slice.len() <= $size);
        let mut a = [0; $size];
        a[..$slice.len()].clone_from_slice(&$slice);
        a
    }};
}

pub fn softmac_key_configuration_from_mlme(
    key_descriptor: fidl_mlme::SetKeyDescriptor,
) -> fidl_softmac::WlanKeyConfiguration {
    fidl_softmac::WlanKeyConfiguration {
        protection: Some(fidl_softmac::WlanProtection::RxTx),
        cipher_oui: Some(key_descriptor.cipher_suite_oui),
        cipher_type: Some(fidl_ieee80211::CipherSuiteType::into_primitive(
            key_descriptor.cipher_suite_type,
        ) as u8),
        key_type: Some(match key_descriptor.key_type {
            fidl_mlme::KeyType::Pairwise => fidl_ieee80211::KeyType::Pairwise,
            fidl_mlme::KeyType::PeerKey => fidl_ieee80211::KeyType::Peer,
            fidl_mlme::KeyType::Igtk => fidl_ieee80211::KeyType::Igtk,
            fidl_mlme::KeyType::Group => fidl_ieee80211::KeyType::Group,
        }),
        peer_addr: Some(key_descriptor.address),
        key_idx: Some(key_descriptor.key_id as u8),
        key: Some(key_descriptor.key),
        rsc: Some(key_descriptor.rsc),
        ..Default::default()
    }
}

pub fn mlme_band_cap_from_softmac(
    band_cap: fidl_softmac::WlanSoftmacBandCapability,
) -> Result<fidl_mlme::BandCapability, anyhow::Error> {
    fn required<T>(field: Option<T>, name: impl Display) -> Result<T, anyhow::Error> {
        field.ok_or_else(|| {
            format_err!("Required band capability field unset in SoftMAC driver FIDL: `{}`.", name)
        })
    }

    Ok(fidl_mlme::BandCapability {
        band: required(band_cap.band, "band")?,
        basic_rates: required(band_cap.basic_rates.clone(), "basic_rates")?.into(),
        primary_channels: required(band_cap.primary_channels.clone(), "primary_channels")?,
        ht_cap: band_cap.ht_caps.clone().map(Box::new),
        vht_cap: band_cap.vht_caps.clone().map(Box::new),
    })
}

pub fn mlme_device_info_from_softmac(
    query_response: fidl_softmac::WlanSoftmacQueryResponse,
) -> Result<fidl_mlme::DeviceInfo, anyhow::Error> {
    fn required<T>(field: Option<T>, name: impl Display) -> Result<T, anyhow::Error> {
        field.ok_or_else(|| {
            format_err!("Required query field unset in SoftMAC driver FIDL: `{}`.", name)
        })
    }

    let band_caps = query_response
        .band_caps
        .as_ref()
        .map(|band_caps| {
            band_caps.iter().cloned().map(mlme_band_cap_from_softmac).collect::<Result<Vec<_>, _>>()
        })
        .transpose()?;
    Ok(fidl_mlme::DeviceInfo {
        sta_addr: required(query_response.sta_addr, "sta_addr")?,
        //TODO(): Replace with factory_addr when it is added to the FIDL.
        factory_addr: required(query_response.sta_addr, "sta_addr")?,
        role: required(query_response.mac_role, "mac_role")?,
        bands: required(band_caps, "band_caps")?,
        qos_capable: false,
        // TODO(https://fxbug.dev/349155104): This seems to only be required for an AP MLME and should not
        // be enforced for clients.
        softmac_hardware_capability: required(
            query_response.hardware_capability,
            "hardware_capability",
        )?,
    })
}

pub fn get_rssi_dbm(rx_info: fidl_softmac::WlanRxInfo) -> Option<i8> {
    if rx_info.valid_fields.contains(fidl_softmac::WlanRxInfoValid::RSSI) && rx_info.rssi_dbm != 0 {
        Some(rx_info.rssi_dbm)
    } else {
        None
    }
}

// TODO(b/308634817): Remove this conversion once CSsid is no longer used for
//                    SSIDs specified for active scan requests.
pub fn cssid_from_ssid_unchecked(ssid: &Vec<u8>) -> fidl_ieee80211::CSsid {
    let mut cssid = fidl_ieee80211::CSsid {
        len: ssid.len() as u8,
        data: [0; fidl_ieee80211::MAX_SSID_BYTE_LEN as usize],
    };
    // Ssid never exceeds fidl_ieee80211::MAX_SSID_BYTE_LEN bytes, so this assignment will never panic
    cssid.data[..ssid.len()].copy_from_slice(&ssid[..]);
    cssid
}

#[cfg(test)]
mod tests {
    use super::*;
    fn empty_rx_info() -> fidl_softmac::WlanRxInfo {
        fidl_softmac::WlanRxInfo {
            rx_flags: fidl_softmac::WlanRxInfoFlags::empty(),
            valid_fields: fidl_softmac::WlanRxInfoValid::empty(),
            phy: fidl_ieee80211::WlanPhyType::Dsss,
            data_rate: 0,
            primary: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
            mcs: 0,
            rssi_dbm: 0,
            snr_dbh: 0,
            bandwidth: fidl_ieee80211::ChannelBandwidth::Cbw20,
            vht_secondary_80_channel: fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: 0,
            },
        }
    }

    #[test]
    fn test_get_rssi_dbm_field_not_valid() {
        let rx_info = fidl_softmac::WlanRxInfo {
            valid_fields: fidl_softmac::WlanRxInfoValid::empty(),
            rssi_dbm: 20,
            ..empty_rx_info()
        };
        assert_eq!(get_rssi_dbm(rx_info), None);
    }

    #[test]
    fn test_get_rssi_dbm_zero_dbm() {
        let rx_info = fidl_softmac::WlanRxInfo {
            valid_fields: fidl_softmac::WlanRxInfoValid::RSSI,
            rssi_dbm: 0,
            ..empty_rx_info()
        };
        assert_eq!(get_rssi_dbm(rx_info), None);
    }

    #[test]
    fn test_get_rssi_dbm_all_good() {
        let rx_info = fidl_softmac::WlanRxInfo {
            valid_fields: fidl_softmac::WlanRxInfoValid::RSSI,
            rssi_dbm: 20,
            ..empty_rx_info()
        };
        assert_eq!(get_rssi_dbm(rx_info), Some(20));
    }

    #[test]
    fn test_mlme_band_cap_from_softmac() {
        let softmac_band_cap = fidl_softmac::WlanSoftmacBandCapability {
            band: Some(fidl_ieee80211::WlanBand::TwoGhz),
            basic_rates: Some(vec![
                0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
            ]),
            primary_channels: Some(
                (1..=14)
                    .map(|c| fidl_ieee80211::ChannelNumber {
                        band: fidl_ieee80211::WlanBand::TwoGhz,
                        number: c,
                    })
                    .collect(),
            ),
            ht_caps: Some(fidl_ieee80211::HtCapabilities {
                bytes: [
                    0x63, 0x00, // HT capability info
                    0x17, // AMPDU params
                    0xff, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                    0x00, // Rx MCS bitmask, Supported MCS values: 0-7
                    0x01, 0x00, 0x00, 0x00, // Tx parameters
                    0x00, 0x00, // HT extended capabilities
                    0x00, 0x00, 0x00, 0x00, // TX beamforming capabilities
                    0x00, // ASEL capabilities
                ],
            }),
            vht_caps: Some(fidl_ieee80211::VhtCapabilities {
                bytes: [
                    0xfe, 0xff, 0xff, 0xff, // VhtCapabilitiesInfo(u32)
                    0xff, 0xaa, 0x00, 0x00, 0x55, 0xff, 0x00, 0x00, // VhtMcsNssSet(u64)
                ],
            }),
            ..Default::default()
        };
        let mlme_band_cap = mlme_band_cap_from_softmac(softmac_band_cap)
            .expect("failed to convert band capability");
        assert_eq!(mlme_band_cap.band, fidl_ieee80211::WlanBand::TwoGhz);
        assert_eq!(
            mlme_band_cap.basic_rates,
            vec![0x02, 0x04, 0x0b, 0x16, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c]
        );
        let expected_channels: Vec<fidl_ieee80211::ChannelNumber> = (1..=14)
            .map(|c| fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: c,
            })
            .collect();
        assert_eq!(mlme_band_cap.primary_channels, expected_channels);
        assert!(mlme_band_cap.ht_cap.is_some());
        assert!(mlme_band_cap.vht_cap.is_some());
    }
}
