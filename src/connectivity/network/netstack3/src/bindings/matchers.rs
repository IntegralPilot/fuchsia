// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::convert::Infallible as Never;

use crate::bindings::util::{IntoCore, TryFromFidl};
use crate::bindings::{BindingsCtx, MatcherBindingsTypes};
use fidl_fuchsia_net_ext::IntoExt as _;
use fidl_fuchsia_net_matchers as fnet_matchers;
use fidl_fuchsia_net_matchers_ext as fnet_matchers_ext;
use netstack3_core::socket::{
    SocketCookieMatcher, SocketTransportProtocolMatcher, TcpSocketMatcher, TcpStateMatcher,
    UdpSocketMatcher, UdpStateMatcher,
};

impl TryFromFidl<fnet_matchers_ext::Interface>
    for netstack3_core::device::InterfaceMatcher<<BindingsCtx as MatcherBindingsTypes>::DeviceClass>
{
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::Interface) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fnet_matchers_ext::Interface::Id(id) => Self::Id(id),
            fnet_matchers_ext::Interface::Name(name) => Self::Name(name),
            fnet_matchers_ext::Interface::PortClass(class) => Self::DeviceClass(class.into()),
        })
    }
}

impl TryFromFidl<fnet_matchers_ext::BoundInterface>
    for netstack3_core::device::BoundInterfaceMatcher<
        <BindingsCtx as MatcherBindingsTypes>::DeviceClass,
    >
{
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::BoundInterface) -> Result<Self, Self::Error> {
        match fidl {
            fnet_matchers_ext::BoundInterface::Bound(matcher) => {
                Ok(netstack3_core::device::BoundInterfaceMatcher::Bound(matcher.into_core()))
            }
            fnet_matchers_ext::BoundInterface::Unbound => {
                Ok(netstack3_core::device::BoundInterfaceMatcher::Unbound)
            }
        }
    }
}

impl TryFromFidl<fnet_matchers_ext::Mark> for netstack3_core::ip::MarkMatcher {
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::Mark) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fnet_matchers_ext::Mark::Unmarked => netstack3_core::ip::MarkMatcher::Unmarked,
            fnet_matchers_ext::Mark::Marked { mask, between, invert } => {
                netstack3_core::ip::MarkMatcher::Marked {
                    mask,
                    start: *between.start(),
                    end: *between.end(),
                    invert,
                }
            }
        })
    }
}

impl TryFromFidl<fnet_matchers_ext::MarkInDomain> for netstack3_core::ip::MarkInDomainMatcher {
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::MarkInDomain) -> Result<Self, Self::Error> {
        let fnet_matchers_ext::MarkInDomain { domain, mark } = fidl;
        Ok(netstack3_core::ip::MarkInDomainMatcher {
            domain: domain.into_core(),
            matcher: mark.into_core(),
        })
    }
}

impl TryFromFidl<fnet_matchers_ext::Port> for netstack3_core::ip::PortMatcher {
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::Port) -> Result<Self, Self::Error> {
        Ok(netstack3_core::ip::PortMatcher { range: fidl.range().clone(), invert: fidl.invert() })
    }
}

impl TryFromFidl<fnet_matchers_ext::BoundPort> for netstack3_core::ip::BoundPortMatcher {
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::BoundPort) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fnet_matchers_ext::BoundPort::Bound(port) => {
                netstack3_core::ip::BoundPortMatcher::Bound(port.into_core())
            }
            fnet_matchers_ext::BoundPort::Unbound => netstack3_core::ip::BoundPortMatcher::Unbound,
        })
    }
}

impl TryFromFidl<fnet_matchers_ext::Address> for netstack3_core::ip::AddressMatcherEither {
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::Address) -> Result<Self, Self::Error> {
        let fnet_matchers_ext::Address { matcher, invert } = fidl;

        let core_matcher = match matcher {
            fnet_matchers_ext::AddressMatcherType::Subnet(subnet) => {
                let subnet: net_types::ip::SubnetEither = subnet.into();

                match subnet {
                    net_types::ip::SubnetEither::V4(subnet) => {
                        Self::V4(netstack3_core::ip::AddressMatcher {
                            matcher: netstack3_core::ip::AddressMatcherType::Subnet(
                                netstack3_core::ip::SubnetMatcher(subnet),
                            ),
                            invert,
                        })
                    }
                    net_types::ip::SubnetEither::V6(subnet) => {
                        Self::V6(netstack3_core::ip::AddressMatcher {
                            matcher: netstack3_core::ip::AddressMatcherType::Subnet(
                                netstack3_core::ip::SubnetMatcher(subnet),
                            ),
                            invert,
                        })
                    }
                }
            }
            fnet_matchers_ext::AddressMatcherType::Range(range) => match range {
                fnet_matchers_ext::AddressRange::V4(range) => {
                    Self::V4(netstack3_core::ip::AddressMatcher {
                        matcher: netstack3_core::ip::AddressMatcherType::Range(
                            (*range.start()).into_ext()..=(*range.end()).into_ext(),
                        ),
                        invert,
                    })
                }
                fnet_matchers_ext::AddressRange::V6(range) => {
                    Self::V6(netstack3_core::ip::AddressMatcher {
                        matcher: netstack3_core::ip::AddressMatcherType::Range(
                            (*range.start()).into_ext()..=(*range.end()).into_ext(),
                        ),
                        invert,
                    })
                }
            },
        };

        Ok(core_matcher)
    }
}

impl TryFromFidl<fnet_matchers_ext::BoundAddress>
    for netstack3_core::ip::BoundAddressMatcherEither
{
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers_ext::BoundAddress) -> Result<Self, Self::Error> {
        Ok(match fidl {
            fnet_matchers_ext::BoundAddress::Bound(address) => {
                netstack3_core::ip::BoundAddressMatcherEither::Bound(address.into_core())
            }
            fnet_matchers_ext::BoundAddress::Unbound => {
                netstack3_core::ip::BoundAddressMatcherEither::Unbound
            }
        })
    }
}

impl TryFromFidl<fnet_matchers_ext::SocketTransportProtocol> for SocketTransportProtocolMatcher {
    type Error = Never;

    fn try_from_fidl(
        fidl: fnet_matchers_ext::SocketTransportProtocol,
    ) -> Result<Self, Self::Error> {
        match fidl {
            fnet_matchers_ext::SocketTransportProtocol::Tcp(tcp) => {
                let matcher = match tcp {
                    fnet_matchers_ext::TcpSocket::Empty => TcpSocketMatcher::Empty,
                    fnet_matchers_ext::TcpSocket::SrcPort(port) => {
                        TcpSocketMatcher::SrcPort(port.into_core())
                    }
                    fnet_matchers_ext::TcpSocket::DstPort(port) => {
                        TcpSocketMatcher::DstPort(port.into_core())
                    }
                    fnet_matchers_ext::TcpSocket::States(states) => {
                        TcpSocketMatcher::State(TcpStateMatcher::from_bits_truncate(states.bits()))
                    }
                };
                Ok(SocketTransportProtocolMatcher::Tcp(matcher))
            }
            fnet_matchers_ext::SocketTransportProtocol::Udp(udp) => {
                let matcher = match udp {
                    fnet_matchers_ext::UdpSocket::Empty => UdpSocketMatcher::Empty,
                    fnet_matchers_ext::UdpSocket::SrcPort(port) => {
                        UdpSocketMatcher::SrcPort(port.into_core())
                    }
                    fnet_matchers_ext::UdpSocket::DstPort(port) => {
                        UdpSocketMatcher::DstPort(port.into_core())
                    }
                    fnet_matchers_ext::UdpSocket::States(states) => {
                        UdpSocketMatcher::State(UdpStateMatcher::from_bits_truncate(states.bits()))
                    }
                };
                Ok(SocketTransportProtocolMatcher::Udp(matcher))
            }
        }
    }
}

impl TryFromFidl<fnet_matchers::SocketCookie> for SocketCookieMatcher {
    type Error = Never;

    fn try_from_fidl(fidl: fnet_matchers::SocketCookie) -> Result<Self, Self::Error> {
        let fnet_matchers::SocketCookie { cookie, invert } = fidl;
        Ok(SocketCookieMatcher { cookie, invert })
    }
}
