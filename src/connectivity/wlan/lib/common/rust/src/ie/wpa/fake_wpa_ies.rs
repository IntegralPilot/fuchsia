// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::ie::rsn::akm::{Akm, PSK};
use crate::ie::rsn::cipher::{Cipher, CCMP_128, TKIP};
use crate::ie::wpa::WpaIe;
use crate::organization::Oui;

pub fn fake_deprecated_wpa1_vendor_ie() -> WpaIe {
    WpaIe {
        unicast_cipher_list: vec![Cipher { oui: Oui::MSFT, suite_type: CCMP_128 }],
        akm_list: vec![Akm { oui: Oui::MSFT, suite_type: PSK }],
        multicast_cipher: Cipher { oui: Oui::MSFT, suite_type: TKIP },
    }
}
