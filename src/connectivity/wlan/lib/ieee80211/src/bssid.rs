// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mac_addr::{MacAddr, MacAddrByteArray, MacAddrBytes, MacFmt, OuiFmt};
use std::fmt;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Unaligned};

// Bssid is a newtype to wrap MacAddrBytes where a BSSID is explicitly required.
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
pub struct Bssid(pub(crate) MacAddrByteArray);

impl MacAddrBytes for Bssid {
    fn to_array(&self) -> MacAddrByteArray {
        self.0
    }

    fn as_array(&self) -> &MacAddrByteArray {
        &self.0
    }
}

impl MacFmt for Bssid {}
impl OuiFmt for Bssid {}

impl From<MacAddrByteArray> for Bssid {
    fn from(bytes: MacAddrByteArray) -> Bssid {
        Bssid(bytes)
    }
}

impl From<MacAddr> for Bssid {
    fn from(mac: MacAddr) -> Bssid {
        Bssid(mac.0)
    }
}

impl fmt::Display for Bssid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_mac_string())
    }
}

impl fmt::Debug for Bssid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bssid({})", self)
    }
}

pub const WILDCARD_BSSID: Bssid = Bssid([0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_bssid_as_mac_string() {
        let bssid = Bssid::from([0x01, 0x02, 0x01, 0x02, 0x01, 0x02]);
        assert_eq!("01:02:01:02:01:02", &format!("{}", bssid));
    }

    #[test]
    fn format_bssid_as_mac_debug_string() {
        let bssid = Bssid::from([0x01, 0x02, 0x01, 0x02, 0x01, 0x02]);
        assert_eq!("Bssid(01:02:01:02:01:02)", &format!("{:?}", bssid));
    }
}
