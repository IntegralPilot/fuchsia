// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// LINT.IfChange

use crate::Bssid;
use anyhow::{Error, format_err};
use fidl_fuchsia_wlan_ieee80211 as fidl_ieee80211;
use std::fmt;
use std::str::FromStr;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

// Strictly speaking, the MAC address is not defined in 802.11, but it's defined
// here for convenience.
pub(crate) type MacAddrByteArray = [u8; fidl_ieee80211::MAC_ADDR_LEN as usize];

pub const BROADCAST_ADDR: MacAddr = MacAddr([0xFF; 6]);
pub const NULL_ADDR: MacAddr = MacAddr([0x00; fidl_ieee80211::MAC_ADDR_LEN as usize]);

#[repr(transparent)]
#[derive(
    KnownLayout,
    FromBytes,
    IntoBytes,
    Immutable,
    Unaligned,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
)]
pub struct MacAddr(pub(crate) MacAddrByteArray);

impl MacAddr {
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// A MAC address is a unicast address if the least significant bit of the first octet is 0.
    /// See "individual/group bit" in
    /// https://standards.ieee.org/content/dam/ieee-standards/standards/web/documents/tutorials/macgrp.pdf
    pub fn is_unicast(&self) -> bool {
        self.0[0] & 1 == 0
    }

    /// IEEE Std 802.3-2015, 3.2.3: The least significant bit of the first octet of a MAC address
    /// denotes multicast.
    pub fn is_multicast(&self) -> bool {
        self.0[0] & 0x01 != 0
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }
}

/// This trait aims to add some friction to convert a type into MacAddrBytes. The purpose being that
/// function using the types implementing this trait, e.g. MacAddr, should prefer not accessing
/// the MacAddrBytes directly when possible.
pub trait MacAddrBytes {
    fn to_array(&self) -> MacAddrByteArray;
    fn as_array(&self) -> &MacAddrByteArray;
}

impl MacAddrBytes for MacAddr {
    fn to_array(&self) -> MacAddrByteArray {
        self.0
    }

    fn as_array(&self) -> &MacAddrByteArray {
        &self.0
    }
}

pub(crate) trait MacFmt {
    fn to_mac_string(&self) -> String
    where
        Self: MacAddrBytes,
    {
        let mac = self.to_array();
        format!(
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        )
    }
}

pub trait OuiFmt {
    fn to_oui_uppercase(&self, sep: &str) -> String
    where
        Self: MacAddrBytes,
    {
        let mac = self.to_array();
        format!("{:02X}{}{:02X}{}{:02X}", mac[0], sep, mac[1], sep, mac[2])
    }
}

impl MacFmt for MacAddr {}
impl OuiFmt for MacAddr {}

impl From<Bssid> for MacAddr {
    fn from(bssid: Bssid) -> MacAddr {
        MacAddr(bssid.0)
    }
}

impl From<MacAddrByteArray> for MacAddr {
    fn from(bytes: MacAddrByteArray) -> MacAddr {
        MacAddr(bytes)
    }
}

impl fmt::Display for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_mac_string())
    }
}

fn detect_delimiter(s: &str) -> Result<char, Error> {
    let contains_semicolon = s.contains(':');
    let contains_hyphen = s.contains('-');
    match (contains_semicolon, contains_hyphen) {
        (true, true) => return Err(format_err!("Either exclusively ':' or '-' must be used.")),
        (false, false) => {
            return Err(format_err!("No valid delimiter found. Only ':' and '-' are supported."));
        }
        (true, false) => Ok(':'),
        (false, true) => Ok('-'),
    }
}

impl FromStr for MacAddr {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut bytes: MacAddrByteArray = [0; 6];
        let mut index = 0;

        let delimiter = detect_delimiter(s)?;
        for octet in s.split(delimiter) {
            if index == 6 {
                return Err(format_err!("Too many octets"));
            }
            bytes[index] = u8::from_str_radix(octet, 16)?;
            index += 1;
        }

        if index != 6 {
            return Err(format_err!("Too few octets. Mixed delimiters are not supported."));
        }
        Ok(MacAddr(bytes))
    }
}

impl fmt::Debug for MacAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "MacAddr({})", self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_mac_addr_as_mac_string() {
        let mac_addr: MacAddr = MacAddr::from([0x00, 0x12, 0x48, 0x9a, 0xbc, 0xdf]);
        assert_eq!("00:12:48:9a:bc:df", &format!("{}", mac_addr));
    }

    #[test]
    fn format_mac_addr_as_mac_debug_string() {
        let mac_addr: MacAddr = MacAddr::from([0x00, 0x12, 0x48, 0x9a, 0xbc, 0xdf]);
        assert_eq!("MacAddr(00:12:48:9a:bc:df)", &format!("{:?}", mac_addr));
    }

    #[test]
    fn format_oui_uppercase() {
        let mac: MacAddr = MacAddr::from([0x0a, 0xb1, 0xcd, 0x9a, 0xbc, 0xdf]);
        assert_eq!(mac.to_oui_uppercase(""), "0AB1CD");
        assert_eq!(mac.to_oui_uppercase(":"), "0A:B1:CD");
        assert_eq!(mac.to_oui_uppercase("-"), "0A-B1-CD");
    }

    #[test]
    fn unicast_addresses() {
        assert!(MacAddr::from([0; 6]).is_unicast());
        assert!(MacAddr::from([0xfe; 6]).is_unicast());
    }

    #[test]
    fn non_unicast_addresses() {
        assert!(!MacAddr::from([0xff; 6]).is_unicast()); // broadcast
        assert!(!MacAddr::from([0x33, 0x33, 0, 0, 0, 0]).is_unicast()); // IPv6 multicast
        assert!(!MacAddr::from([0x01, 0x00, 0x53, 0, 0, 0]).is_unicast()); // IPv4 multicast
    }

    #[test]
    fn is_multicast_valid_addr() {
        assert!(MacAddr::from([33, 33, 33, 33, 33, 33]).is_multicast());
    }

    #[test]
    fn is_multicast_not_valid_addr() {
        assert!(!MacAddr::from([34, 33, 33, 33, 33, 33]).is_multicast());
    }

    #[test]
    fn successfully_parse_mac_str() {
        assert_eq!(
            "01:23:cd:11:11:11".parse::<MacAddr>().unwrap(),
            MacAddr::from([0x01, 0x23, 0xcd, 0x11, 0x11, 0x11])
        );
        assert_eq!(
            "01-23-cd-11-11-11".parse::<MacAddr>().unwrap(),
            MacAddr::from([0x01, 0x23, 0xcd, 0x11, 0x11, 0x11])
        );
        assert_eq!(
            "1-23-cd-11-11-11".parse::<MacAddr>().unwrap(),
            MacAddr::from([0x01, 0x23, 0xcd, 0x11, 0x11, 0x11])
        );
    }

    #[test]
    fn mac_addr_from_str() {
        assert_eq!(
            MacAddr::from_str("01:02:03:ab:cd:ef").unwrap(),
            MacAddr([0x01, 0x02, 0x03, 0xab, 0xcd, 0xef])
        );
        assert_eq!(
            MacAddr::from_str("01-02-03-ab-cd-ef").unwrap(),
            MacAddr([0x01, 0x02, 0x03, 0xab, 0xcd, 0xef])
        );
    }

    #[test]
    fn fail_to_parse_mac_str() {
        assert!("11:11:23::11:11:11".parse::<MacAddr>().is_err());
        assert!("11:11:23:11:11:11:11".parse::<MacAddr>().is_err());
        assert!(":11:23:11:11:11:11".parse::<MacAddr>().is_err());
        assert!("11:23:11:11:11:11:11".parse::<MacAddr>().is_err());
        assert!("11:23:11:11:11:11:".parse::<MacAddr>().is_err());
        assert!("111:23:11:11:11:11".parse::<MacAddr>().is_err());
        assert!("11:23-11:11-11:11".parse::<MacAddr>().is_err());
        assert!("11-23:11-11:11-11".parse::<MacAddr>().is_err());
        assert!("-11-23-11-11-11-11".parse::<MacAddr>().is_err());
        assert!("11-23-11-11-11-11-".parse::<MacAddr>().is_err());
    }
}

// LINT.ThenChange(//src/connectivity/wlan/drivers/lib/macaddr/include/wlan/drivers/macaddr.h)
