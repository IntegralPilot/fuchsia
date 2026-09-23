// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_common as fidl_common;

use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_softmac as fidl_softmac;
use fidl_fuchsia_wlan_tap as wlantap;
use ieee80211::{MacAddr, MacAddrBytes};
use wlan_common::ie::*;
use zerocopy::IntoBytes;

pub(crate) fn create_wlantap_config(
    name: String,
    sta_addr: MacAddr,
    mac_role: fidl_common::WlanMacRole,
) -> wlantap::WlantapPhyConfig {
    wlantap::WlantapPhyConfig {
        // TODO(https://fxbug.dev/42143255): wlantap will configure all of its ifaces to use the same MAC address
        sta_addr: sta_addr.to_array(),
        factory_addr: sta_addr.to_array(),
        supported_phys: vec![
            fidl_ieee80211::WlanPhyType::Dsss,
            fidl_ieee80211::WlanPhyType::Hr,
            fidl_ieee80211::WlanPhyType::Ofdm,
            fidl_ieee80211::WlanPhyType::Erp,
            fidl_ieee80211::WlanPhyType::Ht,
        ],
        mac_role: mac_role,
        hardware_capability: 0,
        bands: vec![create_2_4_ghz_band_info()],
        name,
        quiet: false,
        discovery_support: fidl_softmac::DiscoverySupport {
            scan_offload: Some(fidl_softmac::ScanOffloadExtension {
                supported: Some(true),
                scan_cancel_supported: Some(false),
                ..Default::default()
            }),
            probe_response_offload: Some(fidl_softmac::ProbeResponseOffloadExtension {
                supported: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        },
        mac_sublayer_support: fidl_common::MacSublayerSupport {
            rate_selection_offload: Some(fidl_common::RateSelectionOffloadExtension {
                supported: Some(false),
                ..Default::default()
            }),
            data_plane: Some(fidl_common::DataPlaneExtension {
                data_plane_type: Some(fidl_common::DataPlaneType::EthernetDevice),
                ..Default::default()
            }),
            device: Some(fidl_common::DeviceExtension {
                is_synthetic: Some(true),
                mac_implementation_type: Some(fidl_common::MacImplementationType::Softmac),
                tx_status_report_supported: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        },
        security_support: fidl_common::SecuritySupport {
            sae: Some(fidl_common::SaeFeature {
                driver_handler_supported: Some(false),
                sme_handler_supported: Some(true),
                hash_to_element_supported: Some(false),
                ..Default::default()
            }),
            mfp: Some(fidl_common::MfpFeature { supported: Some(true), ..Default::default() }),
            owe: Some(fidl_common::OweFeature { supported: Some(true), ..Default::default() }),
            ..Default::default()
        },
        spectrum_management_support: fidl_common::SpectrumManagementSupport {
            dfs: Some(fidl_common::DfsFeature { supported: Some(false), ..Default::default() }),
            ..Default::default()
        },
    }
}

fn create_2_4_ghz_band_info() -> wlantap::BandInfo {
    wlantap::BandInfo {
        band: fidl_ieee80211::WlanBand::TwoGhz,
        ht_caps: Some(Box::new(fidl_ieee80211::HtCapabilities {
            bytes: fake_ht_capabilities().as_bytes().try_into().unwrap(),
        })),
        vht_caps: None,
        rates: vec![2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108],
        primary_channels: (1..=14)
            .map(|c| fidl_ieee80211::ChannelNumber {
                band: fidl_ieee80211::WlanBand::TwoGhz,
                number: c,
            })
            .collect(),
    }
}
