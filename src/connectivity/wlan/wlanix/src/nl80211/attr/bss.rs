// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::{Nla, NlaBuffer, NlasIterator};
use netlink_packet_utils::{DecodeError, Emitable, Parseable};
use std::mem::{size_of, size_of_val};

use crate::nl80211::constants::*;

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211BssAttr {
    Bssid([u8; 6]),
    Frequency(u32),
    InformationElement(Vec<u8>),
    LastSeenBoottime(u64),
    SignalMbm(i32),
    Capability(u16),
    Status(Nl80211BssStatus),
    ChainSignal(Vec<ChainSignalAttr>),
}

impl Nla for Nl80211BssAttr {
    fn value_len(&self) -> usize {
        use Nl80211BssAttr::*;
        match self {
            Bssid(val) => size_of_val(val),
            Frequency(val) => size_of_val(val),
            InformationElement(val) => size_of_val(&val[..]),
            LastSeenBoottime(val) => size_of_val(val),
            SignalMbm(val) => size_of_val(val),
            Capability(val) => size_of_val(val),
            Status(_) => size_of::<u32>(),
            ChainSignal(val) => val.as_slice().buffer_len(),
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211BssAttr::*;
        match self {
            Bssid(_) => NL80211_BSS_BSSID,
            Frequency(_) => NL80211_BSS_FREQUENCY,
            InformationElement(_) => NL80211_BSS_INFORMATION_ELEMENTS,
            LastSeenBoottime(_) => NL80211_BSS_LAST_SEEN_BOOTTIME,
            SignalMbm(_) => NL80211_BSS_SIGNAL_MBM,
            Capability(_) => NL80211_BSS_CAPABILITY,
            Status(_) => NL80211_BSS_STATUS,
            ChainSignal(_) => NL80211_BSS_CHAIN_SIGNAL,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211BssAttr::*;
        match self {
            Bssid(val) => buffer.copy_from_slice(&val[..]),
            Frequency(val) => NativeEndian::write_u32(buffer, *val),
            InformationElement(val) => buffer.copy_from_slice(&val[..]),
            LastSeenBoottime(val) => NativeEndian::write_u64(buffer, *val),
            SignalMbm(val) => NativeEndian::write_i32(buffer, *val),
            Capability(val) => NativeEndian::write_u16(buffer, *val),
            Status(val) => NativeEndian::write_u32(buffer, val.into()),
            ChainSignal(val) => val.as_slice().emit(buffer),
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for Nl80211BssAttr {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        use netlink_packet_utils::parsers::{parse_mac, parse_u16, parse_u32};
        let payload = buf.value();
        Ok(match buf.kind() {
            NL80211_BSS_BSSID => Self::Bssid(parse_mac(payload).context("Invalid BSSID")?),
            NL80211_BSS_FREQUENCY => {
                Self::Frequency(parse_u32(payload).context("Invalid frequency")?)
            }
            NL80211_BSS_INFORMATION_ELEMENTS => Self::InformationElement(payload.to_vec()),
            NL80211_BSS_LAST_SEEN_BOOTTIME => {
                if payload.len() != 8 {
                    return Err(DecodeError::from(format!(
                        "Invalid last seen boottime length: {}",
                        payload.len()
                    )));
                }
                Self::LastSeenBoottime(NativeEndian::read_u64(payload))
            }
            NL80211_BSS_SIGNAL_MBM => {
                if payload.len() != 4 {
                    return Err(DecodeError::from(format!(
                        "Invalid signal mbm length: {}",
                        payload.len()
                    )));
                }
                Self::SignalMbm(NativeEndian::read_i32(payload))
            }
            NL80211_BSS_CAPABILITY => {
                Self::Capability(parse_u16(payload).context("Invalid capability")?)
            }
            NL80211_BSS_STATUS => {
                Self::Status(parse_u32(payload).context("Invalid status")?.into())
            }
            NL80211_BSS_CHAIN_SIGNAL => {
                let mut chain_signals = Vec::new();
                for nla in NlasIterator::new(payload) {
                    let nla = nla.map_err(DecodeError::from)?;
                    chain_signals.push(ChainSignalAttr::parse(&nla)?);
                }
                Self::ChainSignal(chain_signals)
            }
            other => return Err(DecodeError::from(format!("Unhandled BSS attribute: {}", other))),
        })
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211BssStatus {
    NotAuthenticated,
    Authenticated,
    Associated,
}

impl From<&Nl80211BssStatus> for u32 {
    fn from(state: &Nl80211BssStatus) -> u32 {
        use Nl80211BssStatus::*;
        match state {
            NotAuthenticated => 0,
            Authenticated => NL80211_BSS_STATUS_AUTHENTICATED,
            Associated => NL80211_BSS_STATUS_ASSOCIATED,
        }
    }
}

impl From<u32> for Nl80211BssStatus {
    fn from(val: u32) -> Self {
        match val {
            NL80211_BSS_STATUS_AUTHENTICATED => Self::Authenticated,
            NL80211_BSS_STATUS_ASSOCIATED => Self::Associated,
            _ => Self::NotAuthenticated,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Debug)]
pub struct ChainSignalAttr {
    pub id: u16,
    pub rssi: i8,
}

impl Nla for ChainSignalAttr {
    fn value_len(&self) -> usize {
        size_of_val(&self.rssi)
    }

    fn kind(&self) -> u16 {
        self.id
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        buffer[0] = self.rssi as u8;
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for ChainSignalAttr {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        if payload.is_empty() {
            return Err(DecodeError::from("ChainSignalAttr payload is empty".to_string()));
        }
        Ok(Self { id: buf.kind(), rssi: payload[0] as i8 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bss_attrs() {
        let attrs = vec![
            Nl80211BssAttr::Bssid([1, 3, 3, 7, 4, 2]),
            Nl80211BssAttr::Frequency(0xaabb),
            Nl80211BssAttr::InformationElement(vec![22, 33, 44]),
            Nl80211BssAttr::LastSeenBoottime(0xccddeeff),
            Nl80211BssAttr::SignalMbm(0x0a0b),
            Nl80211BssAttr::Capability(0x1a1b),
            Nl80211BssAttr::Status(Nl80211BssStatus::Associated),
            Nl80211BssAttr::ChainSignal(vec![
                ChainSignalAttr { id: 99, rssi: -20 },
                ChainSignalAttr { id: 111, rssi: -40 },
            ]),
        ];

        let mut buffer = vec![0; attrs.as_slice().buffer_len()];
        attrs.as_slice().emit(&mut buffer[..]);

        let expected_buffer = vec![
            10, 0, // length
            1, 0, // kind: bssid
            1, 3, 3, 7, 4, 2, // value
            0, 0, // padding
            8, 0, // length
            2, 0, // kind: frequency
            0xbb, 0xaa, 0, 0, // value
            7, 0, // length
            6, 0, // kind: information element
            22, 33, 44, // value
            0,  // padding
            12, 0, // length
            15, 0, // kind: last seen boottime
            0xff, 0xee, 0xdd, 0xcc, 0, 0, 0, 0, // value
            8, 0, // length
            7, 0, // kind: signal mbm
            0x0b, 0x0a, 0, 0, // value
            6, 0, // length
            5, 0, // kind: capability
            0x1b, 0x1a, // value
            0, 0, // padding
            8, 0, // length
            9, 0, // kind: status
            2, 0, 0, 0, // value
            20, 0, // length
            19, 0, // kind
            5, 0, 99, 0, 236, 0, 0, 0, // first chain
            5, 0, 111, 0, 216, 0, 0, 0, // second chain
        ];

        assert_eq!(buffer, expected_buffer);
    }
}
