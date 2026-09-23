// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_wlan_common as fidl_common;

pub fn fake_mac_sublayer_support() -> fidl_common::MacSublayerSupport {
    fidl_common::MacSublayerSupport {
        rate_selection_offload: Some(fidl_common::RateSelectionOffloadExtension {
            supported: Some(false),
            ..Default::default()
        }),
        data_plane: Some(fidl_common::DataPlaneExtension {
            data_plane_type: Some(fidl_common::DataPlaneType::EthernetDevice),
            ..Default::default()
        }),
        device: Some(fidl_common::DeviceExtension {
            is_synthetic: Some(false),
            mac_implementation_type: Some(fidl_common::MacImplementationType::Softmac),
            tx_status_report_supported: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    }
}

pub fn fake_security_support() -> fidl_common::SecuritySupport {
    let mut support = fake_security_support_empty();
    support.mfp.as_mut().unwrap().supported = Some(true);
    support.sae.as_mut().unwrap().sme_handler_supported = Some(true);
    support.owe.as_mut().unwrap().supported = Some(true);
    support
}

pub fn fake_security_support_empty() -> fidl_common::SecuritySupport {
    fidl_common::SecuritySupport {
        mfp: Some(fidl_common::MfpFeature { supported: Some(false), ..Default::default() }),
        sae: Some(fidl_common::SaeFeature {
            driver_handler_supported: Some(false),
            sme_handler_supported: Some(false),
            hash_to_element_supported: Some(false),
            ..Default::default()
        }),
        owe: Some(fidl_common::OweFeature { supported: Some(false), ..Default::default() }),
        ..Default::default()
    }
}

pub fn fake_spectrum_management_support_empty() -> fidl_common::SpectrumManagementSupport {
    fidl_common::SpectrumManagementSupport {
        dfs: Some(fidl_common::DfsFeature { supported: Some(false), ..Default::default() }),
        ..Default::default()
    }
}

pub fn fake_dfs_supported() -> fidl_common::SpectrumManagementSupport {
    fidl_common::SpectrumManagementSupport {
        dfs: Some(fidl_common::DfsFeature { supported: Some(true), ..Default::default() }),
        ..Default::default()
    }
}
