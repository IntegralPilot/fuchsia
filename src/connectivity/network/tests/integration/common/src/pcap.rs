// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! PCAP parsing and validation helpers.

use fuchsia_async as fasync;
use net_types::ip::IpAddress;
use netemul::RealmUdpSocket as _;
use packet::ParsablePacket as _;
use packet_formats::ip::IpExt;
use pcap;

/// Matcher for a UDP packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExpectedUdpPacket<'a, A> {
    /// Source IP address.
    pub src_ip: A,
    /// Destination IP address.
    pub dst_ip: A,
    /// Source UDP port.
    pub src_port: u16,
    /// Destination UDP port.
    pub dst_port: u16,
    /// Expected UDP payload.
    pub payload: &'a [u8],
}

/// Asserts that the packets yielded by `packets_iter` match `expected_packets`.
pub fn assert_udp_packets<A: IpAddress>(
    packets_iter: pcap::PcapNgPacketIter<'_>,
    expected_packets: &[ExpectedUdpPacket<'_, A>],
    force_skip_checksum_validation: bool,
) where
    A::Version: IpExt,
{
    let actual_packets: Vec<ExpectedUdpPacket<'_, A>> = packets_iter
        .map(|epb| {
            let epb = epb.expect("EPB parse error");
            assert_eq!(epb.interface_id, 0);
            assert_eq!(usize::try_from(epb.original_length).unwrap(), epb.packet_data.len());
            assert_eq!(usize::try_from(epb.captured_length).unwrap(), epb.packet_data.len());
            let buf = &epb.packet_data;
            let (mut body, _src_mac, _dst_mac, src_ip, dst_ip, proto, _ttl) =
                packet_formats::testutil::parse_ip_packet_in_ethernet_frame::<A::Version>(
                    &buf,
                    packet_formats::ethernet::EthernetFrameLengthCheck::NoCheck,
                )
                .expect("failed to parse IP packet");

            assert_eq!(proto, packet_formats::ip::IpProto::Udp.into());
            let udp = packet_formats::udp::UdpPacket::parse(
                &mut body,
                packet_formats::udp::UdpParseArgs::with_context(
                    src_ip,
                    dst_ip,
                    packet_formats::testutil::ForceSkipChecksumValidation(
                        force_skip_checksum_validation,
                    ),
                ),
            )
            .expect("failed to parse UDP packet");

            ExpectedUdpPacket {
                src_ip,
                dst_ip,
                src_port: udp.src_port().expect("missing src_port").get(),
                dst_port: udp.dst_port().get(),
                payload: body,
            }
        })
        .collect();

    pretty_assertions::assert_eq!(&actual_packets[..], expected_packets);
}

/// Binds a UDP socket in `realm` to `bind_addr`, sends `payload` to `bind_addr` itself, and
/// receives it.
pub async fn send_udp_to_self_and_recv(
    realm: &netemul::TestRealm<'_>,
    bind_addr: std::net::SocketAddr,
    payload: &[u8],
) {
    let sock =
        fasync::net::UdpSocket::bind_in_realm(realm, bind_addr).await.expect("create socket");
    let sent = sock.send_to(payload, bind_addr).await.expect("send_to failed");
    assert_eq!(sent, payload.len());

    let mut recv_buf = vec![0u8; payload.len()];
    let (received, from_addr) = sock.recv_from(&mut recv_buf).await.expect("recv_from failed");
    assert_eq!(received, payload.len());
    assert_eq!(recv_buf.as_slice(), payload);
    assert_eq!(from_addr, bind_addr);
}
