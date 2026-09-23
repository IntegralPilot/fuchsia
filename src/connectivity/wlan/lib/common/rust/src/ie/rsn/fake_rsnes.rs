// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ie::rsn::akm::AKM_PSK;
use crate::ie::rsn::cipher::{CIPHER_CCMP_128, CIPHER_TKIP};
use crate::ie::rsn::rsne::Rsne;
use fidl_fuchsia_wlan_common as fidl_common;

pub fn fake_wpa2_a_rsne() -> Rsne {
    Rsne {
        group_data_cipher_suite: Some(CIPHER_CCMP_128),
        pairwise_cipher_suites: vec![CIPHER_CCMP_128, CIPHER_TKIP],
        akm_suites: vec![AKM_PSK],
        ..Default::default()
    }
}

use std::sync::LazyLock;
static EMPTY_SECURITY_SUPPORT: LazyLock<fidl_common::SecuritySupport> =
    LazyLock::new(|| fidl_common::SecuritySupport {
        mfp: Some(fidl_common::MfpFeature { supported: Some(false), ..Default::default() }),
        sae: Some(fidl_common::SaeFeature {
            driver_handler_supported: Some(false),
            sme_handler_supported: Some(true),
            hash_to_element_supported: Some(true),
            ..Default::default()
        }),
        owe: Some(fidl_common::OweFeature { supported: Some(false), ..Default::default() }),
        ..Default::default()
    });

pub fn fake_wpa2_s_rsne() -> Rsne {
    fake_wpa2_a_rsne()
        .derive_wpa2_s_rsne(&EMPTY_SECURITY_SUPPORT)
        .expect("Unable to derive supplicant RSNE")
}

pub fn fake_wpa3_a_rsne() -> Rsne {
    Rsne::wpa3_rsne()
}

pub fn fake_wpa3_s_rsne() -> Rsne {
    let mut security_support = EMPTY_SECURITY_SUPPORT.clone();
    security_support.mfp.as_mut().unwrap().supported = Some(true);
    fake_wpa3_a_rsne()
        .derive_wpa3_s_rsne(&security_support)
        .expect("Unable to derive supplicant RSNE")
}
