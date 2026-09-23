// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use regex::Regex;
use std::sync::LazyLock;

// The following regular expressions match a strict subset of the strings
// that match the regular expression defined in //src/developer/forensics/utils/redact/replacer.cc.
pub static MAC_ADDR_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^(?:[0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}$"#).unwrap());
pub static BSSID_REGEX: LazyLock<Regex> = LazyLock::new(|| MAC_ADDR_REGEX.clone());
pub static SSID_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^<ssid-[0-9a-fA-F]{0,64}>$"#).unwrap());

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
    use ieee80211::{Bssid, MacAddr, Ssid};
    use rand::RngExt as _;

    fn random_six_byte_array() -> [u8; 6] {
        rand::random::<[u8; 6]>()
    }

    #[test]
    fn ssid_to_string_shape() {
        let mut rng = rand::rng();
        let ssid = Ssid::from_bytes_unchecked(
            (0..rng.random_range(0..fidl_ieee80211::MAX_SSID_BYTE_LEN as usize))
                .map(|_| rng.random::<u8>())
                .collect::<Vec<u8>>(),
        );
        assert!(SSID_REGEX.is_match(&ssid.to_string()));
    }

    #[test]
    fn test_printed_mac_addr_shape() {
        let mac_addr = MacAddr::from(random_six_byte_array());
        assert!(MAC_ADDR_REGEX.is_match(&format!("{}", mac_addr)));
    }

    #[test]
    fn test_debug_printed_mac_addr_shape() {
        let mac_addr = MacAddr::from(random_six_byte_array());

        let mac_addr_debug_string = format!("{:?}", mac_addr);
        assert!(Regex::new(r#"^MacAddr\([^)]*\)$"#).unwrap().is_match(&mac_addr_debug_string));
        assert!(MAC_ADDR_REGEX.is_match(&mac_addr_debug_string[8..25]));
    }

    #[test]
    fn test_printed_bssid_shape() {
        let bssid = Bssid::from(random_six_byte_array());

        assert!(BSSID_REGEX.is_match(&format!("{}", bssid)));
    }

    #[test]
    fn test_debug_printed_bssid_shape() {
        let bssid = Bssid::from(random_six_byte_array());

        let bssid_debug_string = format!("{:?}", bssid);
        assert!(Regex::new(r#"^Bssid\([^)]*\)$"#).unwrap().is_match(&bssid_debug_string));
        assert!(BSSID_REGEX.is_match(&bssid_debug_string[6..23]));
    }
}
