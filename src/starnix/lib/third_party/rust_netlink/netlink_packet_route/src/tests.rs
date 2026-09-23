// SPDX-License-Identifier: MIT

// This file only contains testing parsing RouteNetlinkMessage, not focusing on
// detailed sub-component parsing. Each component has their own tests module.

use assert_matches::assert_matches;
use netlink_packet_core::{NetlinkHeader, NetlinkMessage, NetlinkPayload};
use netlink_packet_utils::{DecodeError, Emitable};

use crate::address::AddressMessage;
use crate::link::{LinkAttribute, LinkExtentMask, LinkMessage};
use crate::route::{RouteHeader, RouteMessage};
use crate::rule::{RuleHeader, RuleMessage};
use crate::{AddressFamily, RouteNetlinkMessage, RouteNetlinkMessageParseMode};

#[test]
fn test_get_link() {
    // wireshark capture of nlmon against command:
    //   ip link show dev lo
    let raw: Vec<u8> = vec![
        0x30, 0x00, 0x00, 0x00, 0x12, 0x00, 0x01, 0x00, 0xe6, 0x9c, 0x69, 0x65, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x09, 0x00, 0x00, 0x00, 0x07, 0x00, 0x03, 0x00, 0x6c,
        0x6f, 0x00, 0x00,
    ];

    let mut header = NetlinkHeader::default();
    header.length = 48;
    header.message_type = 18;
    header.flags = 1;
    header.sequence_number = 1701420262;

    let expected = NetlinkMessage::new(
        header,
        NetlinkPayload::from(RouteNetlinkMessage::GetLink(LinkMessage {
            attributes: vec![
                LinkAttribute::ExtMask(vec![LinkExtentMask::Vf, LinkExtentMask::SkipStats]),
                LinkAttribute::IfName("lo".to_string()),
            ],
            ..Default::default()
        })),
    );

    assert_eq!(
        NetlinkMessage::deserialize(&raw, RouteNetlinkMessageParseMode::Strict).unwrap(),
        expected
    );
    let mut buffer = vec![0; expected.buffer_len()];
    expected.emit(&mut buffer);
    assert_eq!(buffer.as_slice(), raw);
}

// Note: the following packet captures were performed with an old version of
// iproute2 obtained by checking out
// https://github.com/iproute2/iproute2/commit/b45e300024bb0936a41821ad75117dc08b65669f
// which is the last commit before a series of commits fixing bugs where
// iproute2 would send dump requests for a number of object types, including
// `addr`, `route`, and `rule`, that incorrectly used the interface message
// header and attributes.

#[test]
fn test_parse_malformed_get_route_from_iproute2_relaxed() {
    // wireshark capture of nlmon against command:
    //   ip route show
    let raw: Vec<u8> = vec![
        0x28, 0x00, 0x00, 0x00, 0x1a, 0x00, 0x01, 0x03, 0xb3, 0xc3, 0xa5, 0x69, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    let mut header = NetlinkHeader::default();
    header.length = 40;
    header.message_type = 26;
    header.flags = 0x0301;
    header.sequence_number = 1772471219;

    let expected = NetlinkMessage::new(
        header,
        NetlinkPayload::from(RouteNetlinkMessage::GetRoute(RouteMessage {
            attributes: vec![],
            header: RouteHeader { address_family: AddressFamily::Inet, ..Default::default() },
            ..Default::default()
        })),
    );

    assert_eq!(
        NetlinkMessage::deserialize(&raw, RouteNetlinkMessageParseMode::Relaxed).unwrap(),
        expected
    );
}

#[test]
fn test_parse_malformed_get_route_from_iproute2_strict() {
    // wireshark capture of nlmon against command:
    //   ip route show
    let raw: Vec<u8> = vec![
        0x28, 0x00, 0x00, 0x00, 0x1a, 0x00, 0x01, 0x03, 0xb3, 0xc3, 0xa5, 0x69, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    assert_matches!(
        NetlinkMessage::<RouteNetlinkMessage>::deserialize(
            &raw,
            RouteNetlinkMessageParseMode::Strict
        ),
        Err(DecodeError::FailedToParseMessageWithType { .. })
    );
}

#[test]
fn test_parse_malformed_get_address_from_iproute2_relaxed() {
    // wireshark capture of nlmon against command:
    //   ip addr show
    let raw: Vec<u8> = vec![
        0x28, 0x00, 0x00, 0x00, 0x16, 0x00, 0x01, 0x03, 0x24, 0xce, 0xa5, 0x69, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    let mut header = NetlinkHeader::default();
    header.length = 40;
    header.message_type = 22;
    header.flags = 0x0301;
    header.sequence_number = 1772473892;

    let expected = NetlinkMessage::new(
        header,
        NetlinkPayload::from(RouteNetlinkMessage::GetAddress(AddressMessage::default())),
    );

    assert_eq!(
        NetlinkMessage::deserialize(&raw, RouteNetlinkMessageParseMode::Relaxed).unwrap(),
        expected
    );
}
#[test]
fn test_parse_malformed_get_address_from_iproute2_strict() {
    // wireshark capture of nlmon against command:
    //   ip addr show
    let raw: Vec<u8> = vec![
        0x28, 0x00, 0x00, 0x00, 0x16, 0x00, 0x01, 0x03, 0x24, 0xce, 0xa5, 0x69, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    assert_matches!(
        NetlinkMessage::<RouteNetlinkMessage>::deserialize(
            &raw,
            RouteNetlinkMessageParseMode::Strict
        ),
        Err(DecodeError::FailedToParseMessageWithType { .. })
    );
}

#[test]
fn test_parse_malformed_get_rule_from_iproute2_relaxed() {
    // wireshark capture of nlmon against command:
    //   ip rule show
    let raw: Vec<u8> = vec![
        0x28, 0x00, 0x00, 0x00, 0x22, 0x00, 0x01, 0x03, 0x12, 0xcf, 0xa5, 0x69, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    let mut header = NetlinkHeader::default();
    header.length = 40;
    header.message_type = 34;
    header.flags = 0x0301;
    header.sequence_number = 1772474130;

    let expected = NetlinkMessage::new(
        header,
        NetlinkPayload::from(RouteNetlinkMessage::GetRule(RuleMessage {
            header: RuleHeader { family: AddressFamily::Inet, ..Default::default() },
            ..Default::default()
        })),
    );

    assert_eq!(
        NetlinkMessage::deserialize(&raw, RouteNetlinkMessageParseMode::Relaxed).unwrap(),
        expected
    );
}

#[test]
fn test_parse_malformed_get_rule_from_iproute2_strict() {
    // wireshark capture of nlmon against command:
    //   ip rule show
    let raw: Vec<u8> = vec![
        0x28, 0x00, 0x00, 0x00, 0x22, 0x00, 0x01, 0x03, 0x12, 0xcf, 0xa5, 0x69, 0x00, 0x00, 0x00,
        0x00, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x08, 0x00, 0x1d, 0x00, 0x01, 0x00, 0x00, 0x00,
    ];

    assert_matches!(
        NetlinkMessage::<RouteNetlinkMessage>::deserialize(
            &raw,
            RouteNetlinkMessageParseMode::Strict
        ),
        Err(DecodeError::FailedToParseMessageWithType { .. })
    );
}
