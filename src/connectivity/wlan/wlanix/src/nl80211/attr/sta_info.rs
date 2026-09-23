// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use netlink_packet_utils::Emitable;
use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::Nla;
use std::mem::size_of_val;

use crate::nl80211::constants::*;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211RateInfoAttr {
    Bitrate32(u32),
}

impl Nla for Nl80211RateInfoAttr {
    fn value_len(&self) -> usize {
        use Nl80211RateInfoAttr::*;
        match self {
            Bitrate32(val) => size_of_val(val),
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211RateInfoAttr::*;
        match self {
            Bitrate32(_) => NL80211_RATE_INFO_BITRATE32,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211RateInfoAttr::*;
        match self {
            Bitrate32(val) => NativeEndian::write_u32(buffer, *val),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211StaInfoAttr {
    TxPackets(u32),
    TxFailed(u32),
    RxPackets(u32),
    Signal(i8),
    TxBitrate(Vec<Nl80211RateInfoAttr>),
}

impl Nla for Nl80211StaInfoAttr {
    fn value_len(&self) -> usize {
        use Nl80211StaInfoAttr::*;
        match self {
            TxPackets(val) => size_of_val(val),
            TxFailed(val) => size_of_val(val),
            RxPackets(val) => size_of_val(val),
            Signal(val) => size_of_val(val),
            TxBitrate(attrs) => attrs.as_slice().buffer_len(),
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211StaInfoAttr::*;
        match self {
            TxPackets(_) => NL80211_STA_INFO_TX_PACKETS,
            TxFailed(_) => NL80211_STA_INFO_TX_FAILED,
            RxPackets(_) => NL80211_STA_INFO_RX_PACKETS,
            Signal(_) => NL80211_STA_INFO_SIGNAL,
            TxBitrate(_) => NL80211_STA_INFO_TX_BITRATE,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211StaInfoAttr::*;
        match self {
            TxPackets(val) => NativeEndian::write_u32(buffer, *val),
            TxFailed(val) => NativeEndian::write_u32(buffer, *val),
            RxPackets(val) => NativeEndian::write_u32(buffer, *val),
            Signal(val) => buffer[0] = *val as u8,
            TxBitrate(attrs) => attrs.as_slice().emit(buffer),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use netlink_packet_utils::Emitable;

    #[test]
    fn test_sta_info_attrs() {
        let attrs = vec![
            Nl80211StaInfoAttr::TxPackets(100),
            Nl80211StaInfoAttr::TxFailed(20),
            Nl80211StaInfoAttr::Signal(-40),
        ];

        let mut buffer = vec![0; attrs.as_slice().buffer_len()];
        attrs.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            8, 0, //length
            10, 0, // kind: tx packets
            100, 0, 0, 0, // value
            8, 0, // length
            12, 0, // kind: tx failed
            20, 0, 0, 0, // value
            5, 0, // length
            7, 0,   // kind: signal
            216, // value
            0, 0, 0, // padding
        ];

        assert_eq!(buffer, expected_buffer);
    }
}
