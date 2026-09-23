// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_common as fidl_common;
use fidl_fuchsia_wlan_fullmac as fidl_fullmac;
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use fidl_fuchsia_wlan_sme as fidl_sme;
use test_realm_helpers::constants::DEFAULT_CLIENT_STA_ADDR;
use wlan_common::ie::fake_ht_capabilities;
use zerocopy::IntoBytes;

/// Contains all the configuration required for the fullmac driver.
/// These are primarily used to respond to SME query requests.
/// By default, the configuration is a client with DEFAULT_CLIENT_STA_ADDR that
/// supports 2.4 GHz bands and HT capabilities.
#[derive(Debug, Clone)]
pub struct FullmacDriverConfig {
    pub query_info: fidl_fullmac::WlanFullmacImplQueryResponse,
    pub mac_sublayer_support: fidl_common::MacSublayerSupport,
    pub security_support: fidl_common::SecuritySupport,
    pub spectrum_management_support: fidl_common::SpectrumManagementSupport,
    pub sme_legacy_privacy_support: fidl_sme::LegacyPrivacySupport,
}

impl Default for FullmacDriverConfig {
    /// By default, the driver is configured as a client.
    fn default() -> Self {
        Self {
            query_info: default_fullmac_query_info(),
            mac_sublayer_support: default_mac_sublayer_support(),
            security_support: default_security_support(),
            spectrum_management_support: default_spectrum_management_support(),
            sme_legacy_privacy_support: default_sme_legacy_privacy_support(),
        }
    }
}

impl FullmacDriverConfig {
    pub fn default_ap() -> Self {
        Self {
            query_info: fidl_fullmac::WlanFullmacImplQueryResponse {
                role: Some(fidl_common::WlanMacRole::Ap),
                ..default_fullmac_query_info()
            },
            ..Default::default()
        }
    }
}

pub fn default_fullmac_query_info() -> fidl_fullmac::WlanFullmacImplQueryResponse {
    fidl_fullmac::WlanFullmacImplQueryResponse {
        sta_addr: Some(DEFAULT_CLIENT_STA_ADDR),
        factory_addr: Some(DEFAULT_CLIENT_STA_ADDR),
        role: Some(fidl_common::WlanMacRole::Client),
        band_caps: Some(vec![default_fullmac_band_capability()]),
        ..Default::default()
    }
}

pub fn default_mac_sublayer_support() -> fidl_common::MacSublayerSupport {
    fidl_common::MacSublayerSupport {
        rate_selection_offload: Some(fidl_common::RateSelectionOffloadExtension {
            supported: Some(false),
            ..Default::default()
        }),
        data_plane: Some(fidl_common::DataPlaneExtension {
            data_plane_type: Some(fidl_common::DataPlaneType::GenericNetworkDevice),
            ..Default::default()
        }),
        device: Some(fidl_common::DeviceExtension {
            is_synthetic: Some(false),
            mac_implementation_type: Some(fidl_common::MacImplementationType::Fullmac),
            tx_status_report_supported: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn default_security_support() -> fidl_common::SecuritySupport {
    fidl_common::SecuritySupport {
        sae: Some(fidl_common::SaeFeature {
            driver_handler_supported: Some(false),
            sme_handler_supported: Some(true),
            hash_to_element_supported: Some(false),
            ..Default::default()
        }),
        mfp: Some(fidl_common::MfpFeature { supported: Some(true), ..Default::default() }),
        owe: Some(fidl_common::OweFeature { supported: Some(true), ..Default::default() }),
        ..Default::default()
    }
}

pub fn default_sme_legacy_privacy_support() -> fidl_sme::LegacyPrivacySupport {
    fidl_sme::LegacyPrivacySupport { wep_supported: false, wpa1_supported: false }
}

fn default_spectrum_management_support() -> fidl_common::SpectrumManagementSupport {
    fidl_common::SpectrumManagementSupport {
        dfs: Some(fidl_common::DfsFeature { supported: Some(false), ..Default::default() }),
        ..Default::default()
    }
}

fn default_fullmac_band_capability() -> fidl_fullmac::BandCapability {
    fidl_fullmac::BandCapability {
        band: Some(fidl_ieee80211::WlanBand::TwoGhz),
        basic_rates: Some(vec![2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108]),
        ht_caps: Some(fidl_ieee80211::HtCapabilities {
            bytes: fake_ht_capabilities().as_bytes().try_into().unwrap(),
        }),
        vht_caps: None,
        // By default, the fullmac fake driver supports 2 GHz channels in the US.
        // Specifically, channels 12-14 are avoided or not allowed in the US.
        primary_channels: Some(
            (1..11)
                .map(|c| fidl_ieee80211::ChannelNumber {
                    band: fidl_ieee80211::WlanBand::TwoGhz,
                    number: c,
                })
                .collect(),
        ),
        ..Default::default()
    }
}
