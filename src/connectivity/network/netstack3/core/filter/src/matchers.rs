// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use alloc::sync::Arc;
use core::convert::Infallible as Never;
use core::fmt::Debug;
use netstack3_base::{
    AddressMatcher, InspectableValue, InterfaceMatcher, InterfaceProperties, Matcher,
    MatcherBindingsTypes, PortMatcher,
};

use derivative::Derivative;
use packet_formats::ip::IpExt;

use crate::FilterBindingsTypes;
use crate::logic::Interfaces;
use crate::packets::{FilterIpExt, FilterIpPacket, MaybeTransportPacket, TransportPacketData};
use crate::state::FilterPacketMetadata;

/// A matcher for transport-layer protocol or port numbers.
#[derive(Debug, Clone)]
pub struct TransportProtocolMatcher<P> {
    /// The transport-layer protocol.
    pub proto: P,
    /// If set, the matcher for the source port or identifier of the transport
    /// header.
    pub src_port: Option<PortMatcher>,
    /// If set, the matcher for the destination port or identifier of the
    /// transport header.
    pub dst_port: Option<PortMatcher>,
}

impl<P: Debug> InspectableValue for TransportProtocolMatcher<P> {
    fn record<I: netstack3_base::Inspector>(&self, name: &str, inspector: &mut I) {
        inspector.record_debug(name, self);
    }
}

impl<P: PartialEq, T: MaybeTransportPacket> Matcher<(Option<P>, T)>
    for TransportProtocolMatcher<P>
{
    fn matches(&self, actual: &(Option<P>, T)) -> bool {
        let Self { proto, src_port, dst_port } = self;
        let (packet_proto, packet) = actual;

        let Some(packet_proto) = packet_proto else {
            return false;
        };

        proto == packet_proto && {
            let transport_data = packet.transport_packet_data();
            src_port.required_matches(
                transport_data.as_ref().map(TransportPacketData::src_port).as_ref(),
            ) && dst_port.required_matches(
                transport_data.as_ref().map(TransportPacketData::dst_port).as_ref(),
            )
        }
    }
}

/// A trait for external matchers supplied by bindings.
pub trait BindingsPacketMatcher<D> {
    /// Returns `true` if the packet matches.
    fn matches<I: FilterIpExt, P: FilterIpPacket<I>>(
        &self,
        packet: &P,
        interfaces: Interfaces<'_, D>,
        meta: &impl FilterPacketMetadata,
    ) -> bool;
}

impl<D> BindingsPacketMatcher<D> for Never {
    fn matches<I: FilterIpExt, P: FilterIpPacket<I>>(
        &self,
        _packet: &P,
        _interfaces: Interfaces<'_, D>,
        _meta: &impl FilterPacketMetadata,
    ) -> bool {
        match *self {}
    }
}

impl<D, M: BindingsPacketMatcher<D>> BindingsPacketMatcher<D> for Arc<M> {
    fn matches<I: FilterIpExt, P: FilterIpPacket<I>>(
        &self,
        packet: &P,
        interfaces: Interfaces<'_, D>,
        meta: &impl FilterPacketMetadata,
    ) -> bool {
        self.as_ref().matches(packet, interfaces, meta)
    }
}

/// Top-level matcher for IP packets.
#[derive(Derivative)]
#[derivative(Default(bound = ""), Clone(bound = ""), Debug(bound = ""))]
pub struct PacketMatcher<I: IpExt, BT: MatcherBindingsTypes> {
    /// The interface on which the packet entered the stack.
    ///
    /// Only available in `INGRESS`, `LOCAL_INGRESS`, and `FORWARDING`.
    pub in_interface: Option<InterfaceMatcher<BT::DeviceClass>>,
    /// The interface through which the packet exits the stack.
    ///
    /// Only available in `FORWARDING`, `LOCAL_EGRESS`, and `EGRESS`.
    pub out_interface: Option<InterfaceMatcher<BT::DeviceClass>>,
    /// Matcher for the source IP address.
    pub src_address: Option<AddressMatcher<I::Addr>>,
    /// Matcher for the destination IP address.
    pub dst_address: Option<AddressMatcher<I::Addr>>,
    /// Matchers for the transport layer.
    pub transport_protocol: Option<TransportProtocolMatcher<I::Proto>>,
    /// A custom matcher to run on the packet.
    pub external_matcher: Option<BT::BindingsPacketMatcher>,
}

impl<I, BT> PacketMatcher<I, BT>
where
    I: FilterIpExt,
    BT: FilterBindingsTypes,
{
    pub(crate) fn matches<P, D>(
        &self,
        packet: &P,
        interfaces: Interfaces<'_, D>,
        meta: &impl FilterPacketMetadata,
    ) -> bool
    where
        P: FilterIpPacket<I>,
        D: InterfaceProperties<BT::DeviceClass>,
        BT: FilterBindingsTypes<BindingsPacketMatcher: BindingsPacketMatcher<D>>,
    {
        let Self {
            in_interface,
            out_interface,
            src_address,
            dst_address,
            transport_protocol,
            external_matcher,
        } = self;
        let Interfaces { ingress: in_if, egress: out_if } = interfaces;

        // If no fields are specified, match all traffic by default.
        in_interface.required_matches(in_if)
            && out_interface.required_matches(out_if)
            && src_address.matches(&packet.src_addr())
            && dst_address.matches(&packet.dst_addr())
            && transport_protocol.matches(&(packet.protocol(), packet.maybe_transport_packet()))
            && external_matcher.as_ref().map_or(true, |m| m.matches(packet, interfaces, meta))
    }
}

#[cfg(test)]
mod tests {
    use ip_test_macro::ip_test;
    use net_types::ip::{Ipv4, Ipv4Addr, Ipv6, Ipv6Addr};
    use packet_formats::ip::{IpProto, Ipv4Proto};
    use test_case::test_case;

    use netstack3_base::testutil::{FakeDeviceClass, FakeMatcherDeviceId};
    use netstack3_base::{AddressMatcherType, SegmentHeader, SubnetMatcher};

    use super::*;
    use crate::context::testutil::{FakeBindingsCtx, FakeBindingsPacketMatcher};
    use crate::packets::testutil::internal::{
        ArbitraryValue, FakeIcmpEchoRequest, FakeIpPacket, FakeNullPacket, FakeTcpSegment,
        FakeUdpPacket, TestIpExt, TransportPacketExt,
    };
    use crate::state::FakePacketMetadata;

    #[test_case(InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id))]
    #[test_case(InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name))]
    #[test_case(InterfaceMatcher::DeviceClass(FakeMatcherDeviceId::wlan_interface().class))]
    fn match_on_interface_properties(matcher: InterfaceMatcher<FakeDeviceClass>) {
        let matcher = PacketMatcher::<Ipv4, FakeBindingsCtx<Ipv4>> {
            in_interface: Some(matcher.clone()),
            out_interface: Some(matcher),
            ..Default::default()
        };

        assert_eq!(
            matcher.matches(
                &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                Interfaces {
                    ingress: Some(&FakeMatcherDeviceId::wlan_interface()),
                    egress: Some(&FakeMatcherDeviceId::wlan_interface())
                },
                &FakePacketMetadata::default(),
            ),
            true
        );
        assert_eq!(
            matcher.matches(
                &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                Interfaces {
                    ingress: Some(&FakeMatcherDeviceId::ethernet_interface()),
                    egress: Some(&FakeMatcherDeviceId::ethernet_interface())
                },
                &FakePacketMetadata::default(),
            ),
            false
        );
    }

    #[test_case(InterfaceMatcher::Id(FakeMatcherDeviceId::wlan_interface().id))]
    #[test_case(InterfaceMatcher::Name(FakeMatcherDeviceId::wlan_interface().name))]
    #[test_case(InterfaceMatcher::DeviceClass(FakeMatcherDeviceId::wlan_interface().class))]
    fn interface_matcher_specified_but_not_available_in_hook_does_not_match(
        matcher: InterfaceMatcher<FakeDeviceClass>,
    ) {
        let matcher = PacketMatcher::<Ipv4, FakeBindingsCtx<Ipv4>> {
            in_interface: Some(matcher.clone()),
            out_interface: Some(matcher),
            ..Default::default()
        };

        assert_eq!(
            matcher.matches(
                &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: Some(&FakeMatcherDeviceId::wlan_interface()) },
                &FakePacketMetadata::default(),
            ),
            false,
        );
        assert_eq!(
            matcher.matches(
                &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: Some(&FakeMatcherDeviceId::wlan_interface()), egress: None },
                &FakePacketMetadata::default(),
            ),
            false,
        );
        assert_eq!(
            matcher.matches(
                &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                Interfaces {
                    ingress: Some(&FakeMatcherDeviceId::wlan_interface()),
                    egress: Some(&FakeMatcherDeviceId::wlan_interface())
                },
                &FakePacketMetadata::default(),
            ),
            true,
        );
    }

    enum AddressMatcherTestCase {
        Subnet,
        Range,
    }

    #[ip_test(I)]
    #[test_case(AddressMatcherTestCase::Subnet, /* invert */ false)]
    #[test_case(AddressMatcherTestCase::Subnet, /* invert */ true)]
    #[test_case(AddressMatcherTestCase::Range, /* invert */ false)]
    #[test_case(AddressMatcherTestCase::Range, /* invert */ true)]
    fn match_on_subnet_or_address_range<I: TestIpExt>(
        test_case: AddressMatcherTestCase,
        invert: bool,
    ) {
        let matcher = AddressMatcher {
            matcher: match test_case {
                AddressMatcherTestCase::Subnet => {
                    AddressMatcherType::Subnet(SubnetMatcher(I::SUBNET))
                }
                AddressMatcherTestCase::Range => {
                    // Generate the inclusive address range that is equivalent to the subnet.
                    let start = I::SUBNET.network();
                    let end = I::map_ip(
                        start,
                        |start| {
                            let range_size = 2_u32.pow(32 - u32::from(I::SUBNET.prefix())) - 1;
                            let end = u32::from_be_bytes(start.ipv4_bytes()) + range_size;
                            Ipv4Addr::from(end.to_be_bytes())
                        },
                        |start| {
                            let range_size = 2_u128.pow(128 - u32::from(I::SUBNET.prefix())) - 1;
                            let end = u128::from_be_bytes(start.ipv6_bytes()) + range_size;
                            Ipv6Addr::from(end.to_be_bytes())
                        },
                    );
                    AddressMatcherType::Range(start..=end)
                }
            },
            invert,
        };

        for matcher in [
            PacketMatcher::<I, FakeBindingsCtx<I>> {
                src_address: Some(matcher.clone()),
                ..Default::default()
            },
            PacketMatcher::<I, FakeBindingsCtx<I>> {
                dst_address: Some(matcher),
                ..Default::default()
            },
        ] {
            assert_ne!(
                matcher.matches::<_, FakeMatcherDeviceId>(
                    &FakeIpPacket::<I, FakeTcpSegment>::arbitrary_value(),
                    Interfaces { ingress: None, egress: None },
                    &FakePacketMetadata::default(),
                ),
                invert,
            );
            assert_eq!(
                matcher.matches::<_, FakeMatcherDeviceId>(
                    &FakeIpPacket {
                        src_ip: I::IP_OUTSIDE_SUBNET,
                        dst_ip: I::IP_OUTSIDE_SUBNET,
                        body: FakeTcpSegment::arbitrary_value(),
                    },
                    Interfaces { ingress: None, egress: None },
                    &FakePacketMetadata::default(),
                ),
                invert,
            );
        }
    }

    enum Protocol {
        Tcp,
        Udp,
        Icmp,
    }

    impl Protocol {
        fn ip_proto<I: FilterIpExt>(&self) -> Option<I::Proto> {
            match self {
                Self::Tcp => <&FakeTcpSegment as TransportPacketExt<I>>::proto(),
                Self::Udp => <&FakeUdpPacket as TransportPacketExt<I>>::proto(),
                Self::Icmp => <&FakeIcmpEchoRequest as TransportPacketExt<I>>::proto(),
            }
        }
    }

    #[test_case(Protocol::Tcp, FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value() => true)]
    #[test_case(Protocol::Tcp, FakeIpPacket::<Ipv4, FakeUdpPacket>::arbitrary_value() => false)]
    #[test_case(
        Protocol::Tcp,
        FakeIpPacket::<Ipv4, FakeIcmpEchoRequest>::arbitrary_value()
        => false
    )]
    #[test_case(Protocol::Tcp, FakeIpPacket::<Ipv4, FakeNullPacket>::arbitrary_value() => false)]
    #[test_case(Protocol::Udp, FakeIpPacket::<Ipv4, FakeUdpPacket>::arbitrary_value() => true)]
    #[test_case(Protocol::Udp, FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value()=> false)]
    #[test_case(
        Protocol::Udp,
        FakeIpPacket::<Ipv4, FakeIcmpEchoRequest>::arbitrary_value()
        => false
    )]
    #[test_case(
        Protocol::Icmp,
        FakeIpPacket::<Ipv4, FakeIcmpEchoRequest>::arbitrary_value()
        => true
    )]
    #[test_case(Protocol::Udp, FakeIpPacket::<Ipv4, FakeNullPacket>::arbitrary_value() => false)]
    #[test_case(
        Protocol::Icmp,
        FakeIpPacket::<Ipv6, FakeIcmpEchoRequest>::arbitrary_value()
        => true
    )]
    #[test_case(Protocol::Icmp, FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value() => false)]
    #[test_case(Protocol::Icmp, FakeIpPacket::<Ipv4, FakeUdpPacket>::arbitrary_value() => false)]
    #[test_case(Protocol::Icmp, FakeIpPacket::<Ipv4, FakeNullPacket>::arbitrary_value() => false)]
    fn match_on_transport_protocol<I, P>(protocol: Protocol, packet: P) -> bool
    where
        I: TestIpExt,
        P: FilterIpPacket<I>,
    {
        let matcher = PacketMatcher::<I, FakeBindingsCtx<I>> {
            transport_protocol: Some(TransportProtocolMatcher {
                proto: protocol.ip_proto::<I>().unwrap(),
                src_port: None,
                dst_port: None,
            }),
            ..Default::default()
        };

        matcher.matches::<_, FakeMatcherDeviceId>(
            &packet,
            Interfaces { ingress: None, egress: None },
            &FakePacketMetadata::default(),
        )
    }

    #[test_case(
        Some(PortMatcher { range: 1024..=65535, invert: false }), None, (11111, 80), true;
        "matching src port"
    )]
    #[test_case(
        Some(PortMatcher { range: 1024..=65535, invert: true }), None, (11111, 80), false;
        "invert match src port"
    )]
    #[test_case(
        Some(PortMatcher { range: 1024..=65535, invert: false }), None, (53, 80), false;
        "non-matching src port"
    )]
    #[test_case(
        None, Some(PortMatcher { range: 22..=22, invert: false }), (11111, 22), true;
        "match dst port"
    )]
    #[test_case(
        None, Some(PortMatcher { range: 22..=22, invert: true }), (11111, 22), false;
        "invert match dst port"
    )]
    #[test_case(
        None, Some(PortMatcher { range: 22..=22, invert: false }), (11111, 80), false;
        "non-matching dst port"
    )]
    fn match_on_port_range(
        src_port: Option<PortMatcher>,
        dst_port: Option<PortMatcher>,
        transport_header: (u16, u16),
        expect_match: bool,
    ) {
        // TCP
        let matcher = PacketMatcher::<Ipv4, FakeBindingsCtx<Ipv4>> {
            transport_protocol: Some(TransportProtocolMatcher {
                proto: Ipv4Proto::Proto(IpProto::Tcp),
                src_port: src_port.clone(),
                dst_port: dst_port.clone(),
            }),
            ..Default::default()
        };
        let (src, dst) = transport_header;
        assert_eq!(
            matcher.matches::<_, FakeMatcherDeviceId>(
                &FakeIpPacket::<Ipv4, _> {
                    body: FakeTcpSegment {
                        src_port: src,
                        dst_port: dst,
                        segment: SegmentHeader::arbitrary_value(),
                        payload_len: 8888,
                    },
                    ..ArbitraryValue::arbitrary_value()
                },
                Interfaces { ingress: None, egress: None },
                &FakePacketMetadata::default(),
            ),
            expect_match
        );

        // UDP
        let matcher = PacketMatcher::<Ipv4, FakeBindingsCtx<Ipv4>> {
            transport_protocol: Some(TransportProtocolMatcher {
                proto: Ipv4Proto::Proto(IpProto::Udp),
                src_port,
                dst_port,
            }),
            ..Default::default()
        };
        let (src, dst) = transport_header;
        assert_eq!(
            matcher.matches::<_, FakeMatcherDeviceId>(
                &FakeIpPacket::<Ipv4, _> {
                    body: FakeUdpPacket { src_port: src, dst_port: dst },
                    ..ArbitraryValue::arbitrary_value()
                },
                Interfaces { ingress: None, egress: None },
                &FakePacketMetadata::default(),
            ),
            expect_match
        );
    }

    #[ip_test(I)]
    fn packet_must_match_all_provided_matchers<I: TestIpExt>() {
        let matcher = PacketMatcher::<I, FakeBindingsCtx<I>> {
            src_address: Some(AddressMatcher {
                matcher: AddressMatcherType::Subnet(SubnetMatcher(I::SUBNET)),
                invert: false,
            }),
            dst_address: Some(AddressMatcher {
                matcher: AddressMatcherType::Subnet(SubnetMatcher(I::SUBNET)),
                invert: false,
            }),
            ..Default::default()
        };

        assert_eq!(
            matcher.matches::<_, FakeMatcherDeviceId>(
                &FakeIpPacket::<_, FakeTcpSegment> {
                    src_ip: I::IP_OUTSIDE_SUBNET,
                    ..ArbitraryValue::arbitrary_value()
                },
                Interfaces { ingress: None, egress: None },
                &FakePacketMetadata::default(),
            ),
            false
        );
        assert_eq!(
            matcher.matches::<_, FakeMatcherDeviceId>(
                &FakeIpPacket::<_, FakeTcpSegment> {
                    dst_ip: I::IP_OUTSIDE_SUBNET,
                    ..ArbitraryValue::arbitrary_value()
                },
                Interfaces { ingress: None, egress: None },
                &FakePacketMetadata::default(),
            ),
            false
        );
        assert_eq!(
            matcher.matches::<_, FakeMatcherDeviceId>(
                &FakeIpPacket::<_, FakeTcpSegment>::arbitrary_value(),
                Interfaces { ingress: None, egress: None },
                &FakePacketMetadata::default(),
            ),
            true
        );
    }

    #[test]
    fn match_by_default_if_no_specified_matchers() {
        assert_eq!(
            PacketMatcher::<Ipv4, FakeBindingsCtx<Ipv4>>::default()
                .matches::<_, FakeMatcherDeviceId>(
                    &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                    Interfaces { ingress: None, egress: None },
                    &FakePacketMetadata::default(),
                ),
            true
        );
    }

    #[test_case(true; "yes")]
    #[test_case(false; "no")]
    fn match_external_matcher(result: bool) {
        let external_matcher = FakeBindingsPacketMatcher::new(result);
        assert_eq!(
            PacketMatcher::<Ipv4, FakeBindingsCtx<Ipv4>> {
                external_matcher: Some(external_matcher.clone()),
                ..Default::default()
            }
            .matches::<_, FakeMatcherDeviceId>(
                &FakeIpPacket::<Ipv4, FakeTcpSegment>::arbitrary_value(),
                Interfaces {
                    ingress: Some(&FakeMatcherDeviceId::ethernet_interface()),
                    egress: None
                },
                &FakePacketMetadata::default(),
            ),
            result
        );
        assert_eq!(external_matcher.num_calls(), 1);
    }
}
