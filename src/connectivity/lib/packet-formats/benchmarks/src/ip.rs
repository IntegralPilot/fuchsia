// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fmt::Debug;

use itertools::Itertools as _;
use net_declare::{net_ip_v4, net_ip_v6, net_mac};
use net_types::ethernet::Mac;
use net_types::ip::{Ip, IpVersion, Ipv4, Ipv6};
use packet::{
    BufferAlloc, FragmentedBuffer, NestablePacketBuilder as _, NoOpSerializationContext,
    NoReuseBufferProvider, ReusableBuffer, Serializer,
};
use packet_formats::ethernet::EthernetFrameBuilder;
use packet_formats::ip::{IpPacket as _, IpProto};
use packet_formats::ipv4::options::Ipv4Option;
use packet_formats::ipv4::{Ipv4PacketBuilder, Ipv4PacketBuilderWithOptions};
use packet_formats::ipv6::ext_hdrs::{
    ExtensionHeaderOptionAction, HopByHopOption, HopByHopOptionData, Ipv6ExtensionHeader,
};
use packet_formats::ipv6::{Ipv6PacketBuilder, Ipv6PacketBuilderWithHbhOptions};

pub(crate) const TTL: u8 = 1;
pub(crate) const ROUTER_ALERT: u16 = 1234;

pub(crate) const SRC_MAC: Mac = net_mac!("00:00:00:00:00:01");
pub(crate) const DST_MAC: Mac = net_mac!("00:00:00:00:00:02");

#[derive(Debug, Copy, Clone)]
pub(crate) struct IpBenchmarkConfig {
    /// Wrap the IP packet in an Ethernet packet.
    pub(crate) ethernet: bool,
    /// Include IP options for IPv4 or extension header for IPv6.
    pub(crate) ip_options: bool,
}

impl IpBenchmarkConfig {
    pub(crate) fn combinations() -> impl Iterator<Item = Self> + Clone {
        [true, false]
            .into_iter()
            .cartesian_product([true, false])
            .map(|(ethernet, ip_options)| Self { ethernet, ip_options })
    }

    pub(crate) fn bench_name_particle(&self, ip_version: IpVersion) -> String {
        let Self { ethernet, ip_options } = self;
        let ethernet: &'static str = if *ethernet { "Ethernet" } else { "Raw" };
        let ip_version: &'static str = match ip_version {
            IpVersion::V4 => "IPv4",
            IpVersion::V6 => "IPv6",
        };
        let ip_options: &'static str = if *ip_options { "-options" } else { "" };
        format!("{}/{}{}", ethernet, ip_version, ip_options)
    }
}

#[derive(Debug, Eq, PartialEq)]
#[cfg_attr(not(test), allow(unused))]
pub(crate) struct ExtractedIpInfo<I: IpExt> {
    pub(crate) src_addr: I::Addr,
    pub(crate) dst_addr: I::Addr,
    pub(crate) proto: I::Proto,
    pub(crate) ttl: u8,
    pub(crate) router_alert: Option<u16>,
}

impl<I: IpExt> ExtractedIpInfo<I> {
    /// Returns the expected information matching [`IpExt::make_packet`];
    #[cfg(test)]
    pub(crate) fn expected(ip_options: &IpBenchmarkConfig, proto: I::Proto) -> Self {
        let IpBenchmarkConfig { ethernet: _, ip_options } = ip_options;
        Self {
            src_addr: I::SRC_ADDR,
            dst_addr: I::DST_ADDR,
            proto,
            ttl: TTL,
            router_alert: ip_options.then_some(ROUTER_ALERT),
        }
    }
}

pub(crate) trait IpExt: Ip + packet_formats::ip::IpExt {
    const SRC_ADDR: Self::Addr;
    const DST_ADDR: Self::Addr;

    fn make_packet<
        S: Serializer<NoOpSerializationContext, Buffer: FragmentedBuffer>,
        B: ReusableBuffer,
        A: BufferAlloc<B, Error: Debug>,
    >(
        alloc: A,
        options: &IpBenchmarkConfig,
        proto: IpProto,
        body: S,
    ) -> B;
    fn extract_info(packet: &Self::Packet<&[u8]>) -> ExtractedIpInfo<Self>;
}

impl IpExt for Ipv4 {
    const SRC_ADDR: Self::Addr = net_ip_v4!("192.0.2.1");
    const DST_ADDR: Self::Addr = net_ip_v4!("192.0.2.2");

    fn make_packet<
        S: Serializer<NoOpSerializationContext, Buffer: FragmentedBuffer>,
        B: ReusableBuffer,
        A: BufferAlloc<B, Error: Debug>,
    >(
        alloc: A,
        options: &IpBenchmarkConfig,
        proto: IpProto,
        body: S,
    ) -> B {
        let IpBenchmarkConfig { ethernet, ip_options } = options;
        let b = Ipv4PacketBuilder::new(Self::SRC_ADDR, Self::DST_ADDR, TTL, proto.into());
        if *ip_options {
            let ip = Ipv4PacketBuilderWithOptions::new(
                b,
                [Ipv4Option::RouterAlert { data: ROUTER_ALERT }],
            )
            .unwrap()
            .wrap_body(body);
            maybe_wrap_in_ethernet::<Self, _, _, _>(alloc, *ethernet, ip)
        } else {
            maybe_wrap_in_ethernet::<Self, _, _, _>(alloc, *ethernet, b.wrap_body(body))
        }
    }

    fn extract_info(packet: &Self::Packet<&[u8]>) -> ExtractedIpInfo<Self> {
        ExtractedIpInfo {
            src_addr: packet.src_ip(),
            dst_addr: packet.dst_ip(),
            proto: packet.proto(),
            ttl: packet.ttl(),
            router_alert: packet.iter_options().find_map(|o| match o {
                Ipv4Option::RouterAlert { data } => Some(data),
                _ => None,
            }),
        }
    }
}

impl IpExt for Ipv6 {
    const SRC_ADDR: Self::Addr = net_ip_v6!("2001:db8::1");
    const DST_ADDR: Self::Addr = net_ip_v6!("2001:db8::2");

    fn make_packet<
        S: Serializer<NoOpSerializationContext, Buffer: FragmentedBuffer>,
        B: ReusableBuffer,
        A: BufferAlloc<B, Error: Debug>,
    >(
        alloc: A,
        options: &IpBenchmarkConfig,
        proto: IpProto,
        body: S,
    ) -> B {
        let IpBenchmarkConfig { ethernet, ip_options } = options;
        let b = Ipv6PacketBuilder::new(Self::SRC_ADDR, Self::DST_ADDR, TTL, proto.into());
        if *ip_options {
            let ip = Ipv6PacketBuilderWithHbhOptions::new(
                b,
                [HopByHopOption {
                    action: ExtensionHeaderOptionAction::SkipAndContinue,
                    mutable: false,
                    data: HopByHopOptionData::RouterAlert { data: ROUTER_ALERT },
                }],
            )
            .unwrap()
            .wrap_body(body);
            maybe_wrap_in_ethernet::<Self, _, _, _>(alloc, *ethernet, ip)
        } else {
            maybe_wrap_in_ethernet::<Self, _, _, _>(alloc, *ethernet, b.wrap_body(body))
        }
    }

    fn extract_info(packet: &Self::Packet<&[u8]>) -> ExtractedIpInfo<Self> {
        ExtractedIpInfo {
            src_addr: packet.src_ip(),
            dst_addr: packet.dst_ip(),
            proto: packet.proto(),
            ttl: packet.ttl(),
            router_alert: packet.iter_extension_hdrs().find_map(|o| match o {
                Ipv6ExtensionHeader::HopByHopOptions { options } => {
                    options.iter().find_map(|hbh| match hbh.data {
                        HopByHopOptionData::RouterAlert { data } => Some(data),
                        _ => None,
                    })
                }
                _ => None,
            }),
        }
    }
}

fn maybe_wrap_in_ethernet<
    I: IpExt,
    S: Serializer<NoOpSerializationContext, Buffer: FragmentedBuffer>,
    B: ReusableBuffer,
    A: BufferAlloc<B, Error: Debug>,
>(
    alloc: A,
    wrap: bool,
    body: S,
) -> B {
    if wrap {
        EthernetFrameBuilder::new(SRC_MAC, DST_MAC, I::ETHER_TYPE, 0)
            .wrap_body(body)
            .serialize_outer(&mut NoOpSerializationContext, NoReuseBufferProvider(alloc))
            .map_err(|(e, _)| e)
            .unwrap()
    } else {
        body.serialize_outer(&mut NoOpSerializationContext, NoReuseBufferProvider(alloc))
            .map_err(|(e, _)| e)
            .unwrap()
    }
}
