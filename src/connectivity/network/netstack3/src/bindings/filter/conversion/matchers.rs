// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use net_types::ip::{GenericOverIp, Ip};
use packet_formats::ip::{IpExt, IpProto, Ipv4Proto, Ipv6Proto};

use crate::bindings::ctx::BindingsCtx;
use crate::bindings::filter::controller::{Matchers, SocketFilterProgramWithId};
use crate::bindings::filter::conversion::{
    ConversionError, ConversionResult, IpVersionStrictness, TryConvertToCoreState,
};
use crate::bindings::util::IntoCore;

impl TryConvertToCoreState for Matchers {
    type CoreState<I: IpExt> = netstack3_core::filter::PacketMatcher<I, BindingsCtx>;

    fn try_convert<I: IpExt>(
        self,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<ConversionResult<Self::CoreState<I>>, ConversionError> {
        let Self {
            in_interface,
            out_interface,
            src_addr,
            dst_addr,
            transport_protocol,
            ebpf_program,
        } = self;

        let in_interface = in_interface.map(|matcher| matcher.into_core());
        let out_interface = out_interface.map(|matcher| matcher.into_core());

        let src_address = match src_addr
            .map(|matcher| matcher.try_convert::<I>(ip_version_strictness))
            .transpose()?
        {
            Some(ConversionResult::Omit) => return Ok(ConversionResult::Omit),
            Some(ConversionResult::State(matcher)) => Some(matcher),
            None => None,
        };
        let dst_address = match dst_addr
            .map(|matcher| matcher.try_convert::<I>(ip_version_strictness))
            .transpose()?
        {
            Some(ConversionResult::Omit) => return Ok(ConversionResult::Omit),
            Some(ConversionResult::State(matcher)) => Some(matcher),
            None => None,
        };
        let transport_protocol = match transport_protocol
            .map(|matcher| matcher.try_convert::<I>(ip_version_strictness))
            .transpose()?
        {
            Some(ConversionResult::Omit) => return Ok(ConversionResult::Omit),
            Some(ConversionResult::State(matcher)) => Some(matcher),
            None => None,
        };

        let external_matcher =
            ebpf_program.map(|SocketFilterProgramWithId { id: _, program }| program);

        Ok(ConversionResult::State(netstack3_core::filter::PacketMatcher {
            in_interface,
            out_interface,
            src_address,
            dst_address,
            transport_protocol,
            external_matcher,
        }))
    }
}

impl TryConvertToCoreState for fnet_matchers_ext::Address {
    type CoreState<I: IpExt> = netstack3_core::ip::AddressMatcher<I::Addr>;

    fn try_convert<I: IpExt>(
        self,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<ConversionResult<Self::CoreState<I>>, ConversionError> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        pub(super) struct Wrap<I: IpExt>(
            Result<ConversionResult<netstack3_core::ip::AddressMatcher<I::Addr>>, ConversionError>,
        );

        let either_matcher: netstack3_core::ip::AddressMatcherEither = self.into_core();
        let Wrap(result) = I::map_ip_out::<_, Wrap<I>>(
            either_matcher,
            |either_matcher| match either_matcher {
                netstack3_core::ip::AddressMatcherEither::V4(matcher) => {
                    Wrap(Ok(ConversionResult::State(matcher)))
                }
                netstack3_core::ip::AddressMatcherEither::V6(
                    netstack3_core::ip::AddressMatcher { matcher: _, invert },
                ) => {
                    Wrap(match ip_version_strictness.mismatch_result() {
                        Ok(_) if invert => {
                            // A negative match on an IPv6 address should match
                            // all IPv4 addresses.
                            Ok(ConversionResult::State(
                                netstack3_core::ip::AddressMatcher::match_all(),
                            ))
                        }
                        mismatch => mismatch,
                    })
                }
            },
            |either_matcher| match either_matcher {
                netstack3_core::ip::AddressMatcherEither::V4(
                    netstack3_core::ip::AddressMatcher { matcher: _, invert },
                ) => {
                    Wrap(match ip_version_strictness.mismatch_result() {
                        Ok(_) if invert => {
                            // A negative match on an IPv4 address should match
                            // all IPv6 addresses.
                            Ok(ConversionResult::State(
                                netstack3_core::ip::AddressMatcher::match_all(),
                            ))
                        }
                        mismatch => mismatch,
                    })
                }
                netstack3_core::ip::AddressMatcherEither::V6(matcher) => {
                    Wrap(Ok(ConversionResult::State(matcher)))
                }
            },
        );

        result
    }
}

impl TryConvertToCoreState for fnet_matchers_ext::TransportProtocol {
    type CoreState<I: IpExt> = netstack3_core::filter::TransportProtocolMatcher<I::Proto>;

    fn try_convert<I: IpExt>(
        self,
        ip_version_strictness: IpVersionStrictness,
    ) -> Result<ConversionResult<Self::CoreState<I>>, ConversionError> {
        #[derive(GenericOverIp)]
        #[generic_over_ip(I, Ip)]
        pub(super) struct Wrap<I: IpExt>(Result<ConversionResult<I::Proto>, ConversionError>);

        let matcher = match self {
            fnet_matchers_ext::TransportProtocol::Tcp { src_port, dst_port } => {
                netstack3_core::filter::TransportProtocolMatcher {
                    proto: I::map_ip_out(
                        (),
                        |()| Ipv4Proto::Proto(IpProto::Tcp),
                        |()| Ipv6Proto::Proto(IpProto::Tcp),
                    ),
                    src_port: src_port.map(IntoCore::into_core),
                    dst_port: dst_port.map(IntoCore::into_core),
                }
            }
            fnet_matchers_ext::TransportProtocol::Udp { src_port, dst_port } => {
                netstack3_core::filter::TransportProtocolMatcher {
                    proto: I::map_ip_out(
                        (),
                        |()| Ipv4Proto::Proto(IpProto::Udp),
                        |()| Ipv6Proto::Proto(IpProto::Udp),
                    ),
                    src_port: src_port.map(IntoCore::into_core),
                    dst_port: dst_port.map(IntoCore::into_core),
                }
            }
            fnet_matchers_ext::TransportProtocol::Icmp => {
                let Wrap(result) = I::map_ip_out::<_, Wrap<I>>(
                    (),
                    |()| Wrap(Ok(ConversionResult::State(Ipv4Proto::Icmp))),
                    |()| Wrap(ip_version_strictness.mismatch_result()),
                );
                let proto = match result? {
                    ConversionResult::State(proto) => proto,
                    ConversionResult::Omit => return Ok(ConversionResult::Omit),
                };
                netstack3_core::filter::TransportProtocolMatcher {
                    proto,
                    src_port: None,
                    dst_port: None,
                }
            }
            fnet_matchers_ext::TransportProtocol::Icmpv6 => {
                let Wrap(result) = I::map_ip_out::<_, Wrap<I>>(
                    (),
                    |()| Wrap(ip_version_strictness.mismatch_result()),
                    |()| Wrap(Ok(ConversionResult::State(Ipv6Proto::Icmpv6))),
                );
                let proto = match result? {
                    ConversionResult::State(proto) => proto,
                    ConversionResult::Omit => return Ok(ConversionResult::Omit),
                };
                netstack3_core::filter::TransportProtocolMatcher {
                    proto,
                    src_port: None,
                    dst_port: None,
                }
            }
        };
        Ok(ConversionResult::State(matcher))
    }
}
