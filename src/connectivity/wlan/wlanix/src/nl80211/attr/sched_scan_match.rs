// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::{Nla, NlaBuffer};
use netlink_packet_utils::{DecodeError, Parseable};

use crate::nl80211::constants::*;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211SchedScanMatchAttr {
    Ssid(Vec<u8>),
    Rssi(i32),
    RelativeRssi(i32),
    RssiAdjust(i32),
}

impl Nla for Nl80211SchedScanMatchAttr {
    fn value_len(&self) -> usize {
        use Nl80211SchedScanMatchAttr::*;
        match self {
            Ssid(name) => name.len(),
            Rssi(_) | RelativeRssi(_) | RssiAdjust(_) => std::mem::size_of::<i32>(),
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211SchedScanMatchAttr::*;
        match self {
            Ssid(_) => NL80211_SCHED_SCAN_MATCH_ATTR_SSID,
            Rssi(_) => NL80211_SCHED_SCAN_MATCH_ATTR_RSSI,
            RelativeRssi(_) => NL80211_SCHED_SCAN_MATCH_ATTR_RELATIVE_RSSI,
            RssiAdjust(_) => NL80211_SCHED_SCAN_MATCH_ATTR_RSSI_ADJUST,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211SchedScanMatchAttr::*;
        match self {
            Ssid(val) => buffer.copy_from_slice(val),
            Rssi(val) | RelativeRssi(val) | RssiAdjust(val) => {
                NativeEndian::write_i32(buffer, *val)
            }
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for Nl80211SchedScanMatchAttr {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        Ok(match buf.kind() {
            NL80211_SCHED_SCAN_MATCH_ATTR_SSID => Self::Ssid(payload.to_vec()),
            NL80211_SCHED_SCAN_MATCH_ATTR_RSSI => {
                if payload.len() != 4 {
                    return Err(DecodeError::from(
                        "Invalid NL80211_SCHED_SCAN_MATCH_ATTR_RSSI value",
                    ));
                }
                Self::Rssi(NativeEndian::read_i32(payload))
            }
            NL80211_SCHED_SCAN_MATCH_ATTR_RELATIVE_RSSI => {
                if payload.len() != 4 {
                    return Err(DecodeError::from(
                        "Invalid NL80211_SCHED_SCAN_MATCH_ATTR_RELATIVE_RSSI value",
                    ));
                }
                Self::RelativeRssi(NativeEndian::read_i32(payload))
            }
            NL80211_SCHED_SCAN_MATCH_ATTR_RSSI_ADJUST => {
                if payload.len() != 4 {
                    return Err(DecodeError::from(
                        "Invalid NL80211_SCHED_SCAN_MATCH_ATTR_RSSI_ADJUST value",
                    ));
                }
                Self::RssiAdjust(NativeEndian::read_i32(payload))
            }
            other => {
                return Err(DecodeError::from(format!(
                    "Unhandled NL80211_SCHED_SCAN_MATCH_ATTR: {other}"
                )));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netlink_packet_utils::Emitable;
    use test_case::test_case;

    #[test_case(
        Nl80211SchedScanMatchAttr::Ssid(b"TestSSID".to_vec()),
        vec![12, 0, 1, 0, b'T', b'e', b's', b't', b'S', b'S', b'I', b'D'] ;
        "ssid"
    )]
    #[test_case(
        Nl80211SchedScanMatchAttr::Rssi(-42),
        vec![8, 0, 2, 0, 214, 255, 255, 255] ;
        "rssi"
    )]
    #[test_case(
        Nl80211SchedScanMatchAttr::RelativeRssi(-10),
        vec![8, 0, 3, 0, 246, 255, 255, 255] ;
        "relative_rssi"
    )]
    #[test_case(
        Nl80211SchedScanMatchAttr::RssiAdjust(-5),
        vec![8, 0, 4, 0, 251, 255, 255, 255] ;
        "rssi_adjust"
    )]
    fn emit_and_parse_test(attr: Nl80211SchedScanMatchAttr, bytes: Vec<u8>) {
        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);
        assert_eq!(buffer, bytes);

        let parsed_attr =
            Nl80211SchedScanMatchAttr::parse(&NlaBuffer::new(&bytes).unwrap()).unwrap();
        assert_eq!(parsed_attr, attr);
    }
}
