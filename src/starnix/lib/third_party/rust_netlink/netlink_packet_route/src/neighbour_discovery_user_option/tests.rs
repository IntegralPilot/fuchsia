use crate::neighbour_discovery_user_option::NeighbourDiscoveryIcmpV6Type;
use crate::neighbour_discovery_user_option::header::{
    NeighbourDiscoveryIcmpType, NeighbourDiscoveryUserOptionHeader,
};
use crate::neighbour_discovery_user_option::nla::Nla;
use crate::{RouteNetlinkMessage, RouteNetlinkMessageParseMode};
use netlink_packet_core::{NetlinkHeader, NetlinkMessage};

use super::NeighbourDiscoveryUserOptionMessage;

#[test]
fn nduseropt() {
    // An ND_USEROPT Netlink message like this can be observed via the following steps
    // (h/t Jeff-A-Martin):
    // - Ensure `accept_ra` is enabled on your interface:
    //      sudo sysctl -w net.ipv6.conf.<interface-name>.accept_ra=1
    // - Note that the interface needs to be connected to an IPv6 router.
    // - Create a netlink socket and join the RTNLGRP_NDUSEROPT multicast group. In python:
    //      sock = socket.socket(socket.AF_NETLINK, socket.SOCK_RAW)
    //      sock.bind((0, 0))
    //      # NETLINK_SOL, NETLINK_ADD_MEMBERSHIP, RTNLGRP_NDUSEROPT
    //      sock.setsockopt(270, 1, 20)
    //      print(sock.recv(1024))
    // - In another terminal tab...
    // - Install the `ndisc6` package to get the `rdisc6` command-line tool.
    // - Send a router solicitation on your network by running:
    //      rdisc6 <interface-name>
    // - Observe the ND_USEROPT message printed by the above python script.
    let data: Vec<u8> = vec![
        0x6c, 0x00, 0x00, 0x00, 0x44, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x0a, 0x00, 0x38, 0x00, 0x02, 0x00, 0x00, 0x00, 0x86, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x19, 0x07, 0x00, 0x00, 0x00, 0x12, 0x75, 0x00, 0x20, 0x01, 0x0d, 0xb8, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x20, 0x01, 0x0d, 0xb8,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x53, 0x20, 0x01, 0x0d,
        0xb8, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x53, 0x14, 0x00,
        0x01, 0x00, 0xfe, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x00, 0x00, 0x01,
    ];

    let actual: NetlinkMessage<RouteNetlinkMessage> =
        NetlinkMessage::deserialize(&data[..], RouteNetlinkMessageParseMode::Strict)
            .expect("deserialize netlink message");

    let want: NetlinkMessage<RouteNetlinkMessage> = NetlinkMessage::new(
        {
            let mut header = NetlinkHeader::default();
            header.length = 0x6c;
            header.message_type = 0x44;
            header
        },
        netlink_packet_core::NetlinkPayload::InnerMessage(
            RouteNetlinkMessage::NewNeighbourDiscoveryUserOption(
                NeighbourDiscoveryUserOptionMessage {
                    header: NeighbourDiscoveryUserOptionHeader {
                        interface_index: 2,
                        icmp_type: NeighbourDiscoveryIcmpType::Inet6(
                            NeighbourDiscoveryIcmpV6Type::RouterAdvertisement,
                        ),
                    },
                    option_body:
                    // The bytes here constitute an RDNSS NDP option.
                    vec![
                        0x19, 0x7, 0x0, 0x0, 0x0, 0x12, 0x75, 0x0, 0x20, 0x1, 0xd, 0xb8, 0x0, 0x0,
                        0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x1, 0x20, 0x1, 0xd, 0xb8,
                        0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x1, 0x53, 0x20, 0x1,
                        0xd, 0xb8, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x53,
                    ],
                    attributes: vec![Nla::SourceLinkLocalAddress(vec![
                        0xfe, 0x80, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
                        0x0, 0x1,
                    ])],
                },
            ),
        ),
    );

    assert_eq!(actual, want);

    let mut actual_buf = vec![0u8; actual.buffer_len()];
    actual.serialize(&mut actual_buf);

    let mut want_buf = vec![0u8; want.buffer_len()];
    want.serialize(&mut want_buf);

    assert_eq!(actual_buf, want_buf);
    assert_eq!(actual_buf, data);
}
