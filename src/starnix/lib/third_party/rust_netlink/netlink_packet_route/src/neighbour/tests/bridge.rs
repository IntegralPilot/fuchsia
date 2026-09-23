// SPDX-License-Identifier: MIT

use netlink_packet_utils::{Emitable, ParseableParametrized};

use crate::AddressFamily;
use crate::neighbour::flags::NeighbourFlags;
use crate::neighbour::{
    NeighbourAttribute, NeighbourHeader, NeighbourMessage, NeighbourMessageBuffer, NeighbourState,
};
use crate::route::RouteType;

// wireshark capture(netlink message header removed) of nlmon against command:
//   ip -f bridge neighbour show
#[test]
fn test_bridge_neighbour_show() {
    let raw = vec![
        0x07, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00, 0x00, 0x80, 0x00, 0x02, 0x00, 0x0a, 0x00, 0x02,
        0x00, 0x01, 0x00, 0x5e, 0x00, 0x00, 0x01, 0x00, 0x00,
    ];

    let expected = NeighbourMessage {
        header: NeighbourHeader {
            family: AddressFamily::Bridge,
            ifindex: 3,
            state: NeighbourState::Permanent,
            flags: NeighbourFlags::Own,
            kind: RouteType::Unspec,
        },
        attributes: vec![NeighbourAttribute::LinkLocalAddress(vec![1, 0, 94, 0, 0, 1])],
    };

    assert_eq!(
        expected,
        NeighbourMessage::parse_with_param(
            &NeighbourMessageBuffer::new(&raw).unwrap(),
            crate::RouteNetlinkMessageParseMode::Strict
        )
        .unwrap()
    );

    let mut buf = vec![0; expected.buffer_len()];

    expected.emit(&mut buf);

    assert_eq!(buf, raw);
}
