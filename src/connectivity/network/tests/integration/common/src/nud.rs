// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Useful NUD functions for tests.

use anyhow::Context;
use fidl_fuchsia_net_interfaces_admin as fnet_interfaces_admin;
use fidl_fuchsia_net_interfaces_ext as fnet_interfaces_ext;
use futures::StreamExt as _;
use net_types::SpecifiedAddress as _;

/// Frame metadata of interest to neighbor tests.
#[derive(Debug, Eq, PartialEq)]
pub enum FrameMetadata {
    /// An ARP request or NDP Neighbor Solicitation target address.
    NeighborSolicitation(fidl_fuchsia_net::IpAddress),
    /// A UDP datagram destined to the address.
    Udp(fidl_fuchsia_net::IpAddress),
    /// Any other successfully parsed frame.
    Other,
}

/// Helper function to extract specific frame metadata from a raw Ethernet
/// frame.
///
/// Returns `Err` if the frame can't be parsed or `Ok(FrameMetadata)` with any
/// interesting metadata of interest to neighbor tests.
fn extract_frame_metadata(data: Vec<u8>) -> crate::Result<FrameMetadata> {
    use packet::ParsablePacket;
    use packet_formats::arp::{ArpOp, ArpPacket};
    use packet_formats::ethernet::{EtherType, EthernetFrame, EthernetFrameLengthCheck};
    use packet_formats::icmp::ndp::NdpPacket;
    use packet_formats::icmp::{IcmpParseArgs, Icmpv6Packet};
    use packet_formats::ip::{IpPacket, IpProto, Ipv6Proto};
    use packet_formats::ipv4::Ipv4Packet;
    use packet_formats::ipv6::Ipv6Packet;

    let mut bv = &data[..];
    let ethernet = EthernetFrame::parse(&mut bv, EthernetFrameLengthCheck::NoCheck)
        .context("failed to parse Ethernet frame")?;
    match ethernet
        .ethertype()
        .ok_or_else(|| anyhow::anyhow!("missing ethertype in Ethernet frame"))?
    {
        EtherType::Ipv4 => {
            let ipv4 = Ipv4Packet::parse(&mut bv, ()).context("failed to parse IPv4 packet")?;
            if ipv4.proto() != IpProto::Udp.into() {
                return Ok(FrameMetadata::Other);
            }
            Ok(FrameMetadata::Udp(fidl_fuchsia_net::IpAddress::Ipv4(
                fidl_fuchsia_net::Ipv4Address { addr: ipv4.dst_ip().ipv4_bytes() },
            )))
        }
        EtherType::Arp => {
            let arp = ArpPacket::<_, net_types::ethernet::Mac, net_types::ip::Ipv4Addr>::parse(
                &mut bv,
                (),
            )
            .context("failed to parse ARP packet")?;
            match arp.operation() {
                ArpOp::Request => Ok(FrameMetadata::NeighborSolicitation(
                    fidl_fuchsia_net::IpAddress::Ipv4(fidl_fuchsia_net::Ipv4Address {
                        addr: arp.target_protocol_address().ipv4_bytes(),
                    }),
                )),
                ArpOp::Response => Ok(FrameMetadata::Other),
                ArpOp::Other(other) => Err(anyhow::anyhow!("unrecognized ARP operation {}", other)),
            }
        }
        EtherType::Ipv6 => {
            let ipv6 = Ipv6Packet::parse(&mut bv, ()).context("failed to parse IPv6 packet")?;
            match ipv6.proto() {
                Ipv6Proto::Icmpv6 => {
                    // NB: filtering out packets with an unspecified source address will
                    // filter out DAD-related solicitations.
                    if !ipv6.src_ip().is_specified() {
                        return Ok(FrameMetadata::Other);
                    }
                    let parse_args = IcmpParseArgs::new(ipv6.src_ip(), ipv6.dst_ip());
                    match Icmpv6Packet::parse(&mut bv, parse_args)
                        .context("failed to parse ICMP packet")?
                    {
                        Icmpv6Packet::Ndp(NdpPacket::NeighborSolicitation(solicit)) => {
                            Ok(FrameMetadata::NeighborSolicitation(
                                fidl_fuchsia_net::IpAddress::Ipv6(fidl_fuchsia_net::Ipv6Address {
                                    addr: solicit.message().target_address().ipv6_bytes(),
                                }),
                            ))
                        }
                        _ => Ok(FrameMetadata::Other),
                    }
                }
                Ipv6Proto::Proto(IpProto::Udp) => {
                    Ok(FrameMetadata::Udp(fidl_fuchsia_net::IpAddress::Ipv6(
                        fidl_fuchsia_net::Ipv6Address { addr: ipv6.dst_ip().ipv6_bytes() },
                    )))
                }
                _ => Ok(FrameMetadata::Other),
            }
        }
        EtherType::Other(other) => {
            Err(anyhow::anyhow!("unrecognized ethertype in Ethernet frame {}", other))
        }
    }
}

/// Creates a fake endpoint that extracts [`FrameMetadata`] from exchanged
/// frames in `network`.
pub fn create_metadata_stream<'a>(
    ep: &'a netemul::TestFakeEndpoint<'a>,
) -> impl futures::Stream<Item = crate::Result<FrameMetadata>> + 'a {
    ep.frame_stream().map(|r| {
        let (data, dropped) = r.context("fake_ep FIDL error")?;
        if dropped != 0 {
            Err(anyhow::anyhow!("dropped {} frames on fake endpoint", dropped))
        } else {
            extract_frame_metadata(data)
        }
    })
}

/// Works around CQ timing flakes due to NUD failures.
///
/// Many tests can have flakes reduced by applying this workaround. Typically
/// tests that have more than 1 netstack and use pings or sockets between stacks
/// can observe spurious NUD failures due to infra timing woes. That can be
/// worked around by setting the number of NUD probes to a very high value. Any
/// test that is not directly verifying neighbor behavior can use this
/// workaround to get rid of flakes.
pub async fn apply_nud_flake_workaround(
    control: &fnet_interfaces_ext::admin::Control,
) -> crate::Result {
    control
        .set_configuration(&fnet_interfaces_admin::Configuration {
            ipv4: Some(fnet_interfaces_admin::Ipv4Configuration {
                arp: Some(fnet_interfaces_admin::ArpConfiguration {
                    nud: Some(fnet_interfaces_admin::NudConfiguration {
                        max_multicast_solicitations: Some(u16::MAX),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ipv6: Some(fnet_interfaces_admin::Ipv6Configuration {
                ndp: Some(fnet_interfaces_admin::NdpConfiguration {
                    nud: Some(fnet_interfaces_admin::NudConfiguration {
                        max_multicast_solicitations: Some(u16::MAX),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .map_err(|e| e.into())
        .and_then(|r| {
            r.map(|fnet_interfaces_admin::Configuration { .. }| ())
                .map_err(|e| anyhow::anyhow!("can't set device configuration: {e:?}"))
        })
        .context("apply nud flake workaround")
}
