// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Context;
use netlink_packet_utils::byteorder::{ByteOrder, NativeEndian};
use netlink_packet_utils::nla::{Nla, NlaBuffer, NlasIterator};
use netlink_packet_utils::parsers::{parse_mac, parse_string, parse_u16, parse_u32};
use netlink_packet_utils::{DecodeError, Emitable, Parseable};
use std::mem::size_of_val;

use crate::nl80211::constants::*;
use crate::nl80211::nested::{to_nested_nlas, to_nested_values};

mod band;
mod bss;
mod sched_scan_match;
mod sched_scan_plan;
mod sta_info;

pub use band::*;
pub use bss::*;
pub use sched_scan_match::*;
pub use sched_scan_plan::*;
pub use sta_info::*;

const REG_ALPHA2_LEN: usize = 2;
// Length of the 2-byte code + null-terminating character.
const REG_ALPHA2_NULL_STR_LEN: usize = 3;

// Note: variants are sorted in ascending order by `kind` value.
#[derive(Clone, Eq, PartialEq, Debug)]
pub enum Nl80211Attr {
    Wiphy(u32),
    WiphyName(String),
    IfaceIndex(u32),
    IfaceName(String),
    IfaceType(u32),
    Mac([u8; 6]),
    StaInfo(Vec<Nl80211StaInfoAttr>),
    WiphyBands(Vec<Vec<Nl80211BandAttr>>),
    RegulatoryRegionAlpha2([u8; 2]),
    MaxScanSsids(u8),
    Bss(Vec<Nl80211BssAttr>),
    StatusCode(u16),
    ScanFrequencies(Vec<u32>),
    ScanSsids(Vec<Vec<u8>>),
    Frame(Vec<u8>),
    MaxScheduledScanSsids(u8),
    MaxMatchSets(u8),
    FeatureFlags(u32),
    ScanFlags(u32),
    ProtocolFeatures(u32),
    ExtendedFeatures(Vec<u8>),
    MaxSchedScanPlans(u32),
    MaxScanPlanInterval(u32),
    MaxScanPlanIterations(u32),
    SchedScanPlans(Vec<Vec<Nl80211SchedScanPlanAttr>>),
    SchedScanInterval(u32),
    SchedScanMatch(Vec<Vec<Nl80211SchedScanMatchAttr>>),
    Ie(Vec<u8>),
    RelativeRssi(i32),
    RssiAdjust(i32),
    SchedScanDelay(u32),
    SchedScanMulti(bool),
}

impl Nla for Nl80211Attr {
    fn value_len(&self) -> usize {
        use Nl80211Attr::*;
        match self {
            Wiphy(val) => size_of_val(val),
            WiphyName(name) => name.len() + 1,
            IfaceIndex(val) => size_of_val(val),
            IfaceName(name) => name.len() + 1,
            IfaceType(val) => size_of_val(val),
            Mac(val) => size_of_val(val),
            StaInfo(val) => val.as_slice().buffer_len(),
            WiphyBands(bands) => to_nested_nlas(bands).as_slice().buffer_len(),
            // NL80211_ATTR_REG_ALPHA2 is defined by the Linux kernel as an NLA_STRING.
            // Under Netlink, NLA_STRINGs must include the null-terminating '\0' byte.
            // Android daemons (like wificond) rely on this 3-byte C-string representation.
            RegulatoryRegionAlpha2(_) => REG_ALPHA2_NULL_STR_LEN,
            MaxScanSsids(val) => size_of_val(val),
            ScanFrequencies(val) => to_nested_values(val).as_slice().buffer_len(),
            ScanSsids(val) => to_nested_values(val).as_slice().buffer_len(),
            Frame(val) => val.len(),
            Bss(val) => val.as_slice().buffer_len(),
            StatusCode(val) => size_of_val(val),
            MaxScheduledScanSsids(val) => size_of_val(val),
            MaxMatchSets(val) => size_of_val(val),
            FeatureFlags(val) => size_of_val(val),
            ScanFlags(val) => size_of_val(val),
            ProtocolFeatures(val) => size_of_val(val),
            ExtendedFeatures(val) => val.len(),
            MaxSchedScanPlans(val) => size_of_val(val),
            MaxScanPlanInterval(val) => size_of_val(val),
            MaxScanPlanIterations(val) => size_of_val(val),
            SchedScanPlans(val) => to_nested_nlas(val).as_slice().buffer_len(),
            SchedScanInterval(val) => size_of_val(val),
            SchedScanMatch(val) => to_nested_nlas(val).as_slice().buffer_len(),
            Ie(val) => val.len(),
            RelativeRssi(val) => size_of_val(val),
            RssiAdjust(val) => size_of_val(val),
            SchedScanDelay(val) => size_of_val(val),
            SchedScanMulti(_) => 0,
        }
    }

    fn kind(&self) -> u16 {
        use Nl80211Attr::*;
        match self {
            Wiphy(_) => NL80211_ATTR_WIPHY,
            WiphyName(_) => NL80211_ATTR_WIPHY_NAME,
            IfaceIndex(_) => NL80211_ATTR_IFINDEX,
            IfaceName(_) => NL80211_ATTR_IFNAME,
            IfaceType(_) => NL80211_ATTR_IFTYPE,
            Mac(_) => NL80211_ATTR_MAC,
            StaInfo(_) => NL80211_ATTR_STA_INFO,
            WiphyBands(_) => NL80211_ATTR_WIPHY_BANDS,
            RegulatoryRegionAlpha2(_) => NL80211_ATTR_REG_ALPHA2,
            MaxScanSsids(_) => NL80211_ATTR_MAX_NUM_SCAN_SSIDS,
            ScanFrequencies(_) => NL80211_ATTR_SCAN_FREQUENCIES,
            ScanSsids(_) => NL80211_ATTR_SCAN_SSIDS,
            Frame(_) => NL80211_ATTR_FRAME,
            Bss(_) => NL80211_ATTR_BSS,
            StatusCode(_) => NL80211_ATTR_STATUS_CODE,
            MaxScheduledScanSsids(_) => NL80211_ATTR_MAX_NUM_SCHED_SCAN_SSIDS,
            MaxMatchSets(_) => NL80211_ATTR_MAX_MATCH_SETS,
            FeatureFlags(_) => NL80211_ATTR_FEATURE_FLAGS,
            ScanFlags(_) => NL80211_ATTR_SCAN_FLAGS,
            ProtocolFeatures(_) => NL80211_ATTR_PROTOCOL_FEATURES,
            ExtendedFeatures(_) => NL80211_ATTR_EXT_FEATURES,
            MaxSchedScanPlans(_) => NL80211_ATTR_MAX_NUM_SCHED_SCAN_PLANS,
            MaxScanPlanInterval(_) => NL80211_ATTR_MAX_SCAN_PLAN_INTERVAL,
            MaxScanPlanIterations(_) => NL80211_ATTR_MAX_SCAN_PLAN_ITERATIONS,
            SchedScanPlans(_) => NL80211_ATTR_SCHED_SCAN_PLANS,
            SchedScanInterval(_) => NL80211_ATTR_SCHED_SCAN_INTERVAL,
            SchedScanMatch(_) => NL80211_ATTR_SCHED_SCAN_MATCH,
            Ie(_) => NL80211_ATTR_IE,
            RelativeRssi(_) => NL80211_ATTR_SCHED_SCAN_RELATIVE_RSSI,
            RssiAdjust(_) => NL80211_ATTR_SCHED_SCAN_RSSI_ADJUST,
            SchedScanDelay(_) => NL80211_ATTR_SCHED_SCAN_DELAY,
            SchedScanMulti(_) => NL80211_ATTR_SCHED_SCAN_MULTI,
        }
    }

    fn emit_value(&self, buffer: &mut [u8]) {
        use Nl80211Attr::*;
        // Note: Values are all emitted in host byte order. This is the default
        // for netlink unless the NLA_F_NET_BYTEORDER flag is set, but we do not
        // currently use that feature.
        match self {
            Wiphy(val) => NativeEndian::write_u32(buffer, *val),
            WiphyName(name) => {
                buffer[..name.len()].copy_from_slice(name.as_bytes());
                buffer[name.len()] = 0;
            }
            IfaceIndex(val) => NativeEndian::write_u32(buffer, *val),
            IfaceName(name) => {
                buffer[..name.len()].copy_from_slice(name.as_bytes());
                buffer[name.len()] = 0;
            }
            IfaceType(val) => NativeEndian::write_u32(buffer, *val),
            Mac(val) => buffer.copy_from_slice(&val[..]),
            StaInfo(val) => val.as_slice().emit(buffer),
            WiphyBands(bands) => to_nested_nlas(bands).as_slice().emit(buffer),
            // Encode as a standard null-terminated C-string.
            RegulatoryRegionAlpha2(reg) => {
                buffer[..REG_ALPHA2_LEN].copy_from_slice(&reg[..]);
                buffer[REG_ALPHA2_LEN] = 0;
            }
            MaxScanSsids(val) => buffer[0] = *val,
            ScanFrequencies(val) => to_nested_values(val).as_slice().emit(buffer),
            ScanSsids(val) => to_nested_values(val).as_slice().emit(buffer),
            Frame(val) => buffer.copy_from_slice(&val[..]),
            Bss(val) => val.as_slice().emit(buffer),
            StatusCode(val) => NativeEndian::write_u16(buffer, *val),
            MaxScheduledScanSsids(val) => buffer[0] = *val,
            MaxMatchSets(val) => buffer[0] = *val,
            FeatureFlags(val) => NativeEndian::write_u32(buffer, *val),
            ScanFlags(val) => NativeEndian::write_u32(buffer, *val),
            ProtocolFeatures(val) => NativeEndian::write_u32(buffer, *val),
            ExtendedFeatures(val) => buffer.copy_from_slice(&val[..]),
            MaxSchedScanPlans(val) => NativeEndian::write_u32(buffer, *val),
            MaxScanPlanInterval(val) => NativeEndian::write_u32(buffer, *val),
            MaxScanPlanIterations(val) => NativeEndian::write_u32(buffer, *val),
            SchedScanPlans(val) => to_nested_nlas(val).as_slice().emit(buffer),
            SchedScanInterval(val) => NativeEndian::write_u32(buffer, *val),
            SchedScanMatch(val) => to_nested_nlas(val).as_slice().emit(buffer),
            Ie(val) => buffer.copy_from_slice(&val[..]),
            RelativeRssi(val) => NativeEndian::write_i32(buffer, *val),
            RssiAdjust(val) => NativeEndian::write_i32(buffer, *val),
            SchedScanDelay(val) => NativeEndian::write_u32(buffer, *val),
            SchedScanMulti(_) => {}
        }
    }
}

impl<'a, T: AsRef<[u8]> + ?Sized> Parseable<NlaBuffer<&'a T>> for Nl80211Attr {
    type Error = DecodeError;
    fn parse(buf: &NlaBuffer<&'a T>) -> Result<Self, DecodeError> {
        let payload = buf.value();
        Ok(match buf.kind() {
            NL80211_ATTR_WIPHY => {
                Self::Wiphy(parse_u32(payload).context("Invalid NL80211_ATTR_WIPHY value")?)
            }
            NL80211_ATTR_WIPHY_NAME => Self::WiphyName(
                parse_string(payload).context("Invalid NL80211_ATTR_WIPHY_NAME value")?,
            ),
            NL80211_ATTR_IFINDEX => {
                Self::IfaceIndex(parse_u32(payload).context("Invalid NL80211_ATTR_IFINDEX value")?)
            }
            NL80211_ATTR_IFNAME => {
                Self::IfaceName(parse_string(payload).context("Invalid NL80211_ATTR_IFNAME value")?)
            }
            NL80211_ATTR_IFTYPE => {
                Self::IfaceType(parse_u32(payload).context("Invalid NL80211_ATTR_IFTYPE value")?)
            }
            NL80211_ATTR_MAC => {
                Self::Mac(parse_mac(payload).context("Invalid NL80211_ATTR_MAC value")?)
            }
            NL80211_ATTR_WIPHY_BANDS => {
                // This parse implementation is complicated, so it's deliberately
                // omitted for now. We might need to implement this later if a
                // use-case comes up.
                return Err(DecodeError::from(
                    "WiphyBands attribute has no parse implementation".to_string(),
                ));
            }
            NL80211_ATTR_REG_ALPHA2 => {
                // Support both 2-byte raw values and 3-byte C-strings.
                let is_valid_bytes = payload.len() == REG_ALPHA2_LEN;
                let is_valid_string = payload.len() == REG_ALPHA2_NULL_STR_LEN
                    && payload[REG_ALPHA2_NULL_STR_LEN - 1] == 0;
                if !is_valid_bytes && !is_valid_string {
                    return Err(format!("invalid regulatory region alpha2: {payload:?}").into());
                }
                let mut reg = [0u8; REG_ALPHA2_LEN];
                reg.copy_from_slice(&payload[..REG_ALPHA2_LEN]);
                Self::RegulatoryRegionAlpha2(reg)
            }
            NL80211_ATTR_MAX_NUM_SCAN_SSIDS => Self::MaxScanSsids(payload[0]),
            NL80211_ATTR_SCAN_FREQUENCIES => NlasIterator::new(payload)
                .map(|nla| nla.map_err(DecodeError::from).and_then(|v| parse_u32(v.value())))
                .collect::<Result<Vec<_>, _>>()
                .map(Self::ScanFrequencies)
                .context("Invalid NL80211_ATTR_SCAN_FREQUENCIES value")?,
            NL80211_ATTR_SCAN_SSIDS => NlasIterator::new(payload)
                .map(|nla| nla.map(|v| v.value().to_vec()))
                .collect::<Result<Vec<_>, _>>()
                .map(Self::ScanSsids)
                .context("Invalid NL80211_ATTR_SCAN_SSIDS value")?,
            NL80211_ATTR_FRAME => Self::Frame(payload.to_vec()),
            NL80211_ATTR_BSS => NlasIterator::new(payload)
                .map(|nla| nla.map_err(DecodeError::from).and_then(|v| Nl80211BssAttr::parse(&v)))
                .collect::<Result<Vec<_>, _>>()
                .map(Self::Bss)
                .context("Invalid NL80211_ATTR_BSS value")?,
            NL80211_ATTR_STATUS_CODE => Self::StatusCode(
                parse_u16(payload).context("Invalid NL80211_ATTR_STATUS_CODE value")?,
            ),
            NL80211_ATTR_MAX_NUM_SCHED_SCAN_SSIDS => Self::MaxScheduledScanSsids(payload[0]),
            NL80211_ATTR_MAX_MATCH_SETS => Self::MaxMatchSets(payload[0]),
            NL80211_ATTR_FEATURE_FLAGS => Self::FeatureFlags(
                parse_u32(payload).context("Invalid NL80211_ATTR_FEATURE_FLAGS value")?,
            ),
            NL80211_ATTR_SCAN_FLAGS => Self::ScanFlags(
                parse_u32(payload).context("Invalid NL80211_ATTR_SCAN_FLAGS value")?,
            ),
            NL80211_ATTR_PROTOCOL_FEATURES => Self::ProtocolFeatures(
                parse_u32(payload).context("Invalid NL80211_ATTR_PROTOCOL_FEATURES value")?,
            ),
            NL80211_ATTR_EXT_FEATURES => Self::ExtendedFeatures(payload.to_vec()),
            NL80211_ATTR_MAX_NUM_SCHED_SCAN_PLANS => Self::MaxSchedScanPlans(
                parse_u32(payload)
                    .context("Invalid NL80211_ATTR_MAX_NUM_SCHED_SCAN_PLANS value")?,
            ),
            NL80211_ATTR_MAX_SCAN_PLAN_INTERVAL => Self::MaxScanPlanInterval(
                parse_u32(payload).context("Invalid NL80211_ATTR_MAX_SCAN_PLAN_INTERVAL value")?,
            ),
            NL80211_ATTR_MAX_SCAN_PLAN_ITERATIONS => Self::MaxScanPlanIterations(
                parse_u32(payload)
                    .context("Invalid NL80211_ATTR_MAX_SCAN_PLAN_ITERATIONS value")?,
            ),
            NL80211_ATTR_SCHED_SCAN_PLANS => NlasIterator::new(payload)
                .map(|nla| {
                    nla.map_err(DecodeError::from).and_then(|v| {
                        NlasIterator::new(v.value())
                            .map(|nested| {
                                nested
                                    .map_err(DecodeError::from)
                                    .and_then(|v2| Nl80211SchedScanPlanAttr::parse(&v2))
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Self::SchedScanPlans)
                .context("Invalid NL80211_ATTR_SCHED_SCAN_PLANS value")?,
            NL80211_ATTR_SCHED_SCAN_MATCH => NlasIterator::new(payload)
                .map(|nla| {
                    nla.map_err(DecodeError::from).and_then(|v| {
                        NlasIterator::new(v.value())
                            .map(|nested| {
                                nested
                                    .map_err(DecodeError::from)
                                    .and_then(|v2| Nl80211SchedScanMatchAttr::parse(&v2))
                            })
                            .collect::<Result<Vec<_>, _>>()
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(Self::SchedScanMatch)
                .context("Invalid NL80211_ATTR_SCHED_SCAN_MATCH value")?,
            NL80211_ATTR_SCHED_SCAN_INTERVAL => parse_u32(payload)
                .map(Self::SchedScanInterval)
                .context("Invalid SchedScanInterval")?,
            NL80211_ATTR_IE => Self::Ie(payload.to_vec()),
            NL80211_ATTR_SCHED_SCAN_RELATIVE_RSSI => {
                if payload.len() != 4 {
                    return Err(DecodeError::from("Invalid RelativeRssi length"));
                }
                Self::RelativeRssi(NativeEndian::read_i32(payload))
            }
            NL80211_ATTR_SCHED_SCAN_RSSI_ADJUST => {
                if payload.len() != 4 {
                    return Err(DecodeError::from("Invalid RssiAdjust length"));
                }
                Self::RssiAdjust(NativeEndian::read_i32(payload))
            }
            NL80211_ATTR_SCHED_SCAN_DELAY => {
                parse_u32(payload).map(Self::SchedScanDelay).context("Invalid SchedScanDelay")?
            }
            NL80211_ATTR_SCHED_SCAN_MULTI => {
                let val = if payload.is_empty() { true } else { payload[0] != 0 };
                Self::SchedScanMulti(val)
            }
            other => {
                return Err(DecodeError::from(format!("Unhandled NL80211 attribute: {other}")));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_case::test_case;

    #[test]
    fn test_emit_and_parse_roundtrip() {
        use Nl80211Attr::*;
        let attrs = vec![
            Wiphy(123),
            WiphyName("wiphy name".to_string()),
            IfaceIndex(1),
            IfaceName("iface name".to_string()),
            IfaceType(2),
            Mac([1, 2, 3, 4, 5, 6]),
            // WiphyBands is not parseable right now, so we skip it.
            RegulatoryRegionAlpha2(*b"AB"),
            MaxScanSsids(10),
            ScanFrequencies(vec![1, 2, 3]),
            ScanSsids(vec![]),
            Frame(vec![1, 2, 3, 4]),
            // Bss is not parseable right now, skip.
            MaxScheduledScanSsids(11),
            MaxMatchSets(12),
            FeatureFlags(1234),
            ScanFlags(12345),
            ProtocolFeatures(123456),
            ExtendedFeatures(vec![]),
            MaxSchedScanPlans(13),
            MaxScanPlanInterval(14),
            MaxScanPlanIterations(15),
            SchedScanPlans(vec![]),
            SchedScanInterval(1234),
        ];

        let mut buffer = vec![0; attrs.as_slice().buffer_len()];
        attrs.as_slice().emit(&mut buffer[..]);
        let parsed_attrs = NlasIterator::new(&buffer[..])
            .map(|nla| nla.map_err(DecodeError::from).and_then(|nla| Nl80211Attr::parse(&nla)))
            .collect::<Result<Vec<_>, _>>()
            .expect("Failed to parse attributes");
        assert_eq!(attrs, parsed_attrs);
    }

    #[test_case(
        Nl80211Attr::Wiphy(123),
        vec![8, 0, NL80211_ATTR_WIPHY as u8, 0, 123, 0, 0, 0] ;
        "u32"
    )]
    #[test_case(
        Nl80211Attr::WiphyName("test".to_string()),
        // The first extra 0 byte is required because the string is null-terminated,
        // and the other three are added to pad the NLA to a 4-byte boundary.
        vec![9, 0, NL80211_ATTR_WIPHY_NAME as u8, 0, b't', b'e', b's', b't', 0, 0, 0, 0] ;
        "string"
    )]
    #[test_case(
        Nl80211Attr::Mac([1, 2, 3, 4, 5, 6]),
        vec![10, 0, NL80211_ATTR_MAC as u8, 0, 1, 2, 3, 4, 5, 6, 0, 0] ;
        "mac"
    )]
    #[test_case(
        Nl80211Attr::MaxScanSsids(22),
        vec![5, 0, NL80211_ATTR_MAX_NUM_SCAN_SSIDS as u8, 0, 22, 0, 0, 0] ;
        "u8"
    )]
    #[test_case(
        Nl80211Attr::ScanFrequencies(vec![11, 22]),
        vec![
            20, 0, // length
            NL80211_ATTR_SCAN_FREQUENCIES as u8, 0, // kind
            8, 0, 1, 0, 11, 0, 0, 0, // entry 1
            8, 0, 2, 0, 22, 0, 0, 0, // entry 2
        ] ; "vec of u32"
    )]
    #[test_case(
        Nl80211Attr::ScanSsids(vec![vec![11, 22, 33], vec![44, 55, 66, 77, 88]]),
        vec![
            24, 0, // length
            NL80211_ATTR_SCAN_SSIDS as u8, 0, // kind
            7, 0, 1, 0, 11, 22, 33, 00, // entry 1
            9, 0, 2, 0, 44, 55, 66, 77, 88, 00, 00, 00, // entry 2
        ] ; "vec of vec of u8"
    )]
    #[test_case(
        Nl80211Attr::Frame(vec![11, 22, 33, 44]),
        vec![8, 0, NL80211_ATTR_FRAME as u8, 0, 11, 22, 33, 44] ;
        "frame"
    )]
    #[test_case(
        Nl80211Attr::ExtendedFeatures(vec![11, 22, 33, 44]),
        vec![8, 0, NL80211_ATTR_EXT_FEATURES as u8, 0, 11, 22, 33, 44] ;
        "vec of u8"
    )]
    #[test_case(
        Nl80211Attr::RegulatoryRegionAlpha2(*b"AB"),
        vec![7, 0, NL80211_ATTR_REG_ALPHA2 as u8, 0, b'A', b'B', 0, 0] ;
        "regulatory region alpha2"
    )]
    fn emit_and_parse_test(attr: Nl80211Attr, bytes: Vec<u8>) {
        // Test emitting the attr.
        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);
        assert_eq!(buffer, bytes);

        // Test parsing the bytes.
        let parsed_attr = Nl80211Attr::parse(&NlaBuffer::new(&bytes).expect("failed to parse nla"))
            .expect("Failed to parse NLA");
        assert_eq!(parsed_attr, attr);
    }

    #[test]
    fn emit_wiphy_bands() {
        let attr = Nl80211Attr::WiphyBands(vec![
            vec![Nl80211BandAttr::HtCapable],
            vec![Nl80211BandAttr::VhtCapable],
        ]);
        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);

        #[rustfmt::skip]
        let expected_buffer = vec![
            20, 0, // length
            NL80211_ATTR_WIPHY_BANDS as u8, 0, // kind
            8, 0, 1, 0, // entry 1 header
            4, 0, NL80211_BAND_ATTR_HT_CAPA as u8, 0, // ht capable
            8, 0, 2, 0, // entry 2 header
            4, 0, NL80211_BAND_ATTR_VHT_CAPA as u8, 0, // vht capable
        ];
        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn emit_bss() {
        let attr = Nl80211Attr::Bss(vec![
            Nl80211BssAttr::Bssid([11, 22, 33, 44, 55, 66]),
            Nl80211BssAttr::Frequency(0xaabbccdd),
        ]);

        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);

        #[rustfmt::skip]
        let expected_buffer = vec![
            24, 0, // length
            NL80211_ATTR_BSS as u8, 0, // kind
            10, 0, // bssid entry: length
            1, 0, // bssid entry: kind
            11, 22, 33, 44, 55, 66, // bssid entry: value
            0, 0, // bssid entry: padding
            8, 0, // frequency entry: length
            2, 0, // frequency entry: kind
            0xdd, 0xcc, 0xbb, 0xaa, // frequency entry: value
        ];
        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn emit_sched_scan_match() {
        use crate::nl80211::Nl80211SchedScanMatchAttr;
        let attr = Nl80211Attr::SchedScanMatch(vec![vec![
            Nl80211SchedScanMatchAttr::Ssid(b"TestSSID".to_vec()),
            Nl80211SchedScanMatchAttr::Rssi(-42),
        ]]);
        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);

        #[rustfmt::skip]
        let expected_buffer = vec![
            28, 0, // length
            NL80211_ATTR_SCHED_SCAN_MATCH as u8, 0, // kind
            24, 0, 1, 0, // entry 1 header
            12, 0, 1, 0, b'T', b'e', b's', b't', b'S', b'S', b'I', b'D', // ssid
            8, 0, 2, 0, 214, 255, 255, 255, // rssi
        ];
        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn emit_sched_scan_plans() {
        use crate::nl80211::Nl80211SchedScanPlanAttr;
        let attr = Nl80211Attr::SchedScanPlans(vec![vec![
            Nl80211SchedScanPlanAttr::Interval(100),
            Nl80211SchedScanPlanAttr::Iterations(5),
        ]]);
        let mut buffer = vec![0; attr.buffer_len()];
        attr.emit(&mut buffer[..]);

        #[rustfmt::skip]
        let expected_buffer = vec![
            24, 0, // length
            NL80211_ATTR_SCHED_SCAN_PLANS as u8, 0, // kind
            20, 0, 1, 0, // entry 1 header
            8, 0, NL80211_SCHED_SCAN_PLAN_INTERVAL as u8, 0, 100, 0, 0, 0, // interval
            8, 0, NL80211_SCHED_SCAN_PLAN_ITERATIONS as u8, 0, 5, 0, 0, 0, // iterations
        ];
        assert_eq!(buffer, expected_buffer);
    }

    #[test]
    fn parse_regulatory_region_alpha2_2_bytes_is_valid() {
        // 2-byte payload (without null terminator).
        let bytes = [6, 0, NL80211_ATTR_REG_ALPHA2 as u8, 0, b'U', b'S', 0, 0];
        let nla = NlaBuffer::new(&bytes[..]).expect("failed to create nla buffer");
        assert_eq!(
            Nl80211Attr::parse(&nla).expect("failed to parse"),
            Nl80211Attr::RegulatoryRegionAlpha2(*b"US")
        );
    }

    #[test]
    fn parse_regulatory_region_alpha2_3_bytes_is_valid() {
        // 3-byte payload with null terminator.
        let bytes = [7, 0, NL80211_ATTR_REG_ALPHA2 as u8, 0, b'U', b'S', 0, 0];
        let nla = NlaBuffer::new(&bytes[..]).expect("failed to create nla buffer");
        assert_eq!(
            Nl80211Attr::parse(&nla).expect("failed to parse"),
            Nl80211Attr::RegulatoryRegionAlpha2(*b"US")
        );
    }

    #[test]
    fn parse_regulatory_region_alpha2_non_null_terminated_is_invalid() {
        // 3-byte payload without null terminator (invalid).
        let bytes = [7, 0, NL80211_ATTR_REG_ALPHA2 as u8, 0, b'U', b'S', b'A', 0];
        let nla = NlaBuffer::new(&bytes[..]).expect("failed to create nla buffer");
        assert!(Nl80211Attr::parse(&nla).is_err());
    }

    #[test]
    fn parse_regulatory_region_alpha2_1_byte_is_invalid() {
        // 1-byte payload (invalid length).
        let bytes = [5, 0, NL80211_ATTR_REG_ALPHA2 as u8, 0, b'U', 0, 0, 0];
        let nla = NlaBuffer::new(&bytes[..]).expect("failed to create nla buffer");
        assert!(Nl80211Attr::parse(&nla).is_err());
    }

    #[test]
    fn parse_regulatory_region_alpha2_4_bytes_is_invalid() {
        // 4-byte payload (invalid length).
        let bytes = [8, 0, NL80211_ATTR_REG_ALPHA2 as u8, 0, b'U', b'S', 0, 0];
        let nla = NlaBuffer::new(&bytes[..]).expect("failed to create nla buffer");
        assert!(Nl80211Attr::parse(&nla).is_err());
    }
}
